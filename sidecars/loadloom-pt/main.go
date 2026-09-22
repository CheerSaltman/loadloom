package main

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"time"

	"github.com/anacrolix/torrent"
	"github.com/anacrolix/torrent/metainfo"
	"golang.org/x/time/rate"

	"github.com/CheerSaltman/loadloom/sidecars/loadloom-pt/peertracker"
	"github.com/CheerSaltman/loadloom/sidecars/loadloom-pt/ramstore"
)

const (
	protocolVersion = 1
	maxMetainfoSize = 4 << 20
)

type options struct {
	source          string
	connections     int
	ramMiB          int
	durationSeconds int
	maxDownloadGiB  float64
	rateMiB         float64
	stalledSeconds  int
}

type emitter struct {
	mu      sync.Mutex
	encoder *json.Encoder
}

func (e *emitter) send(value any) {
	e.mu.Lock()
	defer e.mu.Unlock()
	if err := e.encoder.Encode(value); err != nil {
		fmt.Fprintln(os.Stderr, "encode event:", err)
	}
}

type baseEvent struct {
	Type string `json:"type"`
}

type logEvent struct {
	Type    string `json:"type"`
	Level   string `json:"level"`
	Code    string `json:"code"`
	Message string `json:"message"`
}

type metricsEvent struct {
	Type               string  `json:"type"`
	Seq                uint64  `json:"seq"`
	Phase              string  `json:"phase"`
	Name               string  `json:"name"`
	InfoHash           string  `json:"infoHash"`
	ElapsedSecs        float64 `json:"elapsedSecs"`
	ProgressPercent    float64 `json:"progressPercent"`
	TotalBytes         int64   `json:"totalBytes"`
	WireBytes          int64   `json:"wireBytes"`
	VerifiedBytes      int64   `json:"verifiedBytes"`
	WastedBytes        int64   `json:"wastedBytes"`
	SpeedBps           float64 `json:"speedBps"`
	AverageBps         float64 `json:"averageBps"`
	ActivePeers        int     `json:"activePeers"`
	PendingPeers       int     `json:"pendingPeers"`
	HalfOpenPeers      int     `json:"halfOpenPeers"`
	ConnectedSeeders   int     `json:"connectedSeeders"`
	UsefulPeers        int     `json:"usefulPeers"`
	StalledPeers       int     `json:"stalledPeers"`
	PeerHandshakes     int64   `json:"peerHandshakes"`
	ClosedPeers        int64   `json:"closedPeers"`
	DeadPeers          int64   `json:"deadPeers"`
	DeadPeerPercent    float64 `json:"deadPeerPercent"`
	TrackerErrors      int64   `json:"trackerErrors"`
	TrackerSuccesses   int64   `json:"trackerSuccesses"`
	GoodPieces         int64   `json:"goodPieces"`
	BadPieces          int64   `json:"badPieces"`
	RamUsedBytes       int64   `json:"ramUsedBytes"`
	RamPeakBytes       int64   `json:"ramPeakBytes"`
	RamLimitBytes      int64   `json:"ramLimitBytes"`
	StorageErrors      int64   `json:"storageErrors"`
	PressureLevel      string  `json:"pressureLevel"`
	PressureReason     string  `json:"pressureReason"`
	PayloadPersistence string  `json:"payloadPersistence"`
}

func main() {
	var opts options
	flag.StringVar(&opts.source, "source", "", "magnet URI or HTTP(S) .torrent URL")
	flag.IntVar(&opts.connections, "connections", 180, "maximum established peer connections")
	flag.IntVar(&opts.ramMiB, "ram-mib", 512, "maximum RAM used for unverified pieces")
	flag.IntVar(&opts.durationSeconds, "duration-seconds", 0, "stop after this many seconds; 0 disables")
	flag.Float64Var(&opts.maxDownloadGiB, "max-download-gib", 0, "stop after useful payload reaches GiB; 0 disables")
	flag.Float64Var(&opts.rateMiB, "rate-mib", 0, "download rate limit in MiB/s; 0 is unlimited")
	flag.IntVar(&opts.stalledSeconds, "stalled-seconds", 15, "seconds without useful data before a peer is stalled")
	flag.Parse()

	out := &emitter{encoder: json.NewEncoder(os.Stdout)}
	if err := validateOptions(opts); err != nil {
		out.send(logEvent{Type: "fatal", Level: "error", Code: "PT-001", Message: err.Error()})
		os.Exit(2)
	}
	out.send(map[string]any{
		"type":               "ready",
		"protocolVersion":    protocolVersion,
		"payloadPersistence": "ram-verify-discard",
	})

	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()
	go readCommands(os.Stdin, cancel, out)
	if err := run(ctx, opts, out); errors.Is(err, context.Canceled) {
		out.send(map[string]any{"type": "stopped", "reason": "stopped by controller"})
	} else if err != nil {
		out.send(logEvent{Type: "fatal", Level: "error", Code: "PT-002", Message: err.Error()})
		os.Exit(1)
	}
}

func validateOptions(opts options) error {
	if !strings.HasPrefix(opts.source, "magnet:?") &&
		!strings.HasPrefix(opts.source, "https://") &&
		!strings.HasPrefix(opts.source, "http://") {
		return errors.New("source must be a magnet URI or HTTP(S) .torrent URL")
	}
	if len(opts.source) > 64<<10 {
		return errors.New("source is too long")
	}
	if opts.connections < 8 || opts.connections > 500 {
		return errors.New("connections must be between 8 and 500")
	}
	if opts.ramMiB < 64 || opts.ramMiB > 4096 {
		return errors.New("ram-mib must be between 64 and 4096")
	}
	if opts.durationSeconds < 0 || opts.durationSeconds > 24*60*60 {
		return errors.New("duration-seconds must be between 0 and 86400")
	}
	if opts.maxDownloadGiB < 0 || opts.maxDownloadGiB > 1024 {
		return errors.New("max-download-gib must be between 0 and 1024")
	}
	if opts.rateMiB < 0 || opts.rateMiB > 4096 {
		return errors.New("rate-mib must be between 0 and 4096")
	}
	if opts.stalledSeconds < 5 || opts.stalledSeconds > 120 {
		return errors.New("stalled-seconds must be between 5 and 120")
	}
	return nil
}

func run(ctx context.Context, opts options, out *emitter) error {
	ramLimit := int64(opts.ramMiB) << 20
	ram, err := ramstore.New(ramLimit)
	if err != nil {
		return err
	}
	peers := peertracker.New(time.Duration(opts.stalledSeconds) * time.Second)
	config := torrent.NewDefaultClientConfig()
	config.DefaultStorage = ram
	config.NoUpload = true
	config.Seed = false
	config.DisableWebseeds = true
	config.ExtendedHandshakeClientVersion = "LoadLoom PT RAM probe/0.1"
	config.Bep20 = "-LL0100-"
	config.EstablishedConnsPerTorrent = opts.connections
	config.HalfOpenConnsPerTorrent = min(opts.connections/2, 100)
	config.TotalHalfOpenConns = min(opts.connections, 160)
	config.TorrentPeersLowWater = opts.connections
	config.TorrentPeersHighWater = min(opts.connections*5, 2500)
	config.KeepAliveTimeout = 30 * time.Second
	config.NominalDialTimeout = 12 * time.Second
	config.MinDialTimeout = 3 * time.Second
	config.HandshakesTimeout = 6 * time.Second
	config.MaxUnverifiedBytes = min(ramLimit/2, 256<<20)
	config.DropDuplicatePeerIds = true
	config.Slogger = slog.New(slog.NewTextHandler(io.Discard, nil))
	config.DialRateLimiter = rate.NewLimiter(50, 100)
	if opts.rateMiB > 0 {
		bytesPerSecond := opts.rateMiB * (1 << 20)
		config.DownloadRateLimiter = rate.NewLimiter(rate.Limit(bytesPerSecond), 4<<20)
	}
	peers.Install(config)

	client, err := torrent.NewClient(config)
	if err != nil {
		return fmt.Errorf("create torrent client: %w", err)
	}
	defer client.Close()

	t, err := addSource(ctx, client, opts.source)
	if err != nil {
		return err
	}
	t.SetMaxEstablishedConns(opts.connections)
	var storageErrors atomic.Int64
	t.SetOnWriteChunkError(func(err error) {
		storageErrors.Add(1)
		out.send(logEvent{Type: "log", Level: "error", Code: "PT-201", Message: err.Error()})
	})
	out.send(logEvent{Type: "log", Level: "info", Code: "PT-010", Message: "torrent added; discovering metadata and peers"})

	metadataTimeout := time.NewTimer(90 * time.Second)
	defer metadataTimeout.Stop()
	select {
	case <-ctx.Done():
		return ctx.Err()
	case <-metadataTimeout.C:
		return errors.New("timed out waiting for torrent metadata")
	case <-t.GotInfo():
	}
	t.DownloadAll()
	out.send(logEvent{
		Type:  "log",
		Level: "info",
		Code:  "PT-011",
		Message: fmt.Sprintf(
			"metadata ready: %s, %.2f GiB, %d pieces; payload is RAM-only and discarded after verification",
			t.Name(), float64(t.Length())/(1<<30), t.NumPieces(),
		),
	})

	started := time.Now()
	lastTick := started
	lastUseful := int64(0)
	lastProgress := started
	seq := uint64(0)
	ticker := time.NewTicker(500 * time.Millisecond)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case now := <-ticker.C:
			stats := t.Stats()
			storageStats := ram.Stats()
			peerStats := peers.Sample(t.PeerConns(), now)
			usefulBytes := stats.BytesReadUsefulData.Int64()
			if usefulBytes > lastUseful {
				lastProgress = now
			}
			interval := now.Sub(lastTick).Seconds()
			speed := float64(usefulBytes-lastUseful) / max(interval, 0.001)
			elapsed := now.Sub(started)
			deadPercent := 0.0
			if peerStats.Closed > 0 {
				deadPercent = float64(peerStats.Dead) / float64(peerStats.Closed) * 100
			}
			progress := 0.0
			if t.Length() > 0 {
				progress = float64(storageStats.VerifiedBytes) / float64(t.Length()) * 100
			}
			wasted := max(stats.BytesReadData.Int64()-usefulBytes, 0)
			level, reason := pressureState(
				elapsed,
				stats.ActivePeers,
				peerStats.Stalled,
				peerStats.Dead,
				peerStats.Closed,
				now.Sub(lastProgress),
				storageStats.UsedBytes,
				ramLimit,
				storageErrors.Load(),
			)
			seq++
			out.send(metricsEvent{
				Type:               "metrics",
				Seq:                seq,
				Phase:              "downloading",
				Name:               t.Name(),
				InfoHash:           t.InfoHash().HexString(),
				ElapsedSecs:        elapsed.Seconds(),
				ProgressPercent:    min(progress, 100),
				TotalBytes:         usefulBytes,
				WireBytes:          stats.BytesRead.Int64(),
				VerifiedBytes:      storageStats.VerifiedBytes,
				WastedBytes:        wasted,
				SpeedBps:           speed,
				AverageBps:         float64(usefulBytes) / max(elapsed.Seconds(), 0.001),
				ActivePeers:        stats.ActivePeers,
				PendingPeers:       stats.PendingPeers,
				HalfOpenPeers:      stats.HalfOpenPeers,
				ConnectedSeeders:   stats.ConnectedSeeders,
				UsefulPeers:        peerStats.Useful,
				StalledPeers:       peerStats.Stalled,
				PeerHandshakes:     peerStats.Handshakes,
				ClosedPeers:        peerStats.Closed,
				DeadPeers:          peerStats.Dead,
				DeadPeerPercent:    deadPercent,
				TrackerErrors:      peerStats.TrackerErrors,
				TrackerSuccesses:   peerStats.TrackerSuccesses,
				GoodPieces:         storageStats.GoodPieces,
				BadPieces:          storageStats.BadPieces,
				RamUsedBytes:       storageStats.UsedBytes,
				RamPeakBytes:       storageStats.PeakBytes,
				RamLimitBytes:      ramLimit,
				StorageErrors:      storageErrors.Load(),
				PressureLevel:      level,
				PressureReason:     reason,
				PayloadPersistence: "ram-verify-discard",
			})
			lastTick = now
			lastUseful = usefulBytes

			switch {
			case t.Complete().Bool():
				out.send(map[string]any{"type": "completed", "reason": "all pieces verified and discarded"})
				return nil
			case opts.durationSeconds > 0 && elapsed >= time.Duration(opts.durationSeconds)*time.Second:
				out.send(map[string]any{"type": "completed", "reason": "duration limit reached"})
				return nil
			case opts.maxDownloadGiB > 0 && float64(usefulBytes) >= opts.maxDownloadGiB*(1<<30):
				out.send(map[string]any{"type": "completed", "reason": "download byte limit reached"})
				return nil
			}
		}
	}
}

func addSource(ctx context.Context, client *torrent.Client, source string) (*torrent.Torrent, error) {
	if strings.HasPrefix(source, "magnet:?") {
		t, err := client.AddMagnet(source)
		if err != nil {
			return nil, fmt.Errorf("add magnet: %w", err)
		}
		return t, nil
	}
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, source, nil)
	if err != nil {
		return nil, fmt.Errorf("create metainfo request: %w", err)
	}
	request.Header.Set("User-Agent", "LoadLoom-PT/0.1")
	response, err := (&http.Client{Timeout: 30 * time.Second}).Do(request)
	if err != nil {
		return nil, fmt.Errorf("download metainfo: %w", err)
	}
	defer response.Body.Close()
	if response.StatusCode < 200 || response.StatusCode >= 300 {
		return nil, fmt.Errorf("download metainfo: HTTP %s", response.Status)
	}
	payload, err := io.ReadAll(io.LimitReader(response.Body, maxMetainfoSize+1))
	if err != nil {
		return nil, fmt.Errorf("read metainfo: %w", err)
	}
	if len(payload) > maxMetainfoSize {
		return nil, fmt.Errorf("metainfo exceeds %d bytes", maxMetainfoSize)
	}
	meta, err := metainfo.Load(bytes.NewReader(payload))
	if err != nil {
		return nil, fmt.Errorf("decode metainfo: %w", err)
	}
	t, err := client.AddTorrent(meta)
	if err != nil {
		return nil, fmt.Errorf("add torrent: %w", err)
	}
	return t, nil
}

func readCommands(reader io.Reader, cancel context.CancelFunc, out *emitter) {
	scanner := bufio.NewScanner(reader)
	scanner.Buffer(make([]byte, 1024), 64<<10)
	for scanner.Scan() {
		var command struct {
			Type string `json:"type"`
		}
		if err := json.Unmarshal(scanner.Bytes(), &command); err != nil {
			out.send(logEvent{Type: "log", Level: "warn", Code: "PT-101", Message: "ignored malformed controller command"})
			continue
		}
		if command.Type == "stop" {
			cancel()
			return
		}
	}
}
