// Package peertracker summarizes peer connection churn without retaining peer
// payloads or addresses. A peer is considered dead when it closes after
// delivering less than 64 KiB of useful data, and stalled when an established
// connection has made no useful progress for the configured interval.
package peertracker

import (
	"sync"
	"sync/atomic"
	"time"

	"github.com/anacrolix/torrent"
)

const deadUsefulBytes = 64 << 10

type state struct {
	connectedAt  time.Time
	lastProgress time.Time
	usefulBytes  int64
}

type Snapshot struct {
	Handshakes       int64
	Closed           int64
	Dead             int64
	Stalled          int
	Useful           int
	TrackerErrors    int64
	TrackerSuccesses int64
}

type Tracker struct {
	mu               sync.Mutex
	peers            map[*torrent.PeerConn]*state
	handshakes       atomic.Int64
	closed           atomic.Int64
	dead             atomic.Int64
	trackerErrors    atomic.Int64
	trackerSuccesses atomic.Int64
	stalledAfter     time.Duration
}

func New(stalledAfter time.Duration) *Tracker {
	return &Tracker{
		peers:        make(map[*torrent.PeerConn]*state),
		stalledAfter: stalledAfter,
	}
}

func (t *Tracker) Install(config *torrent.ClientConfig) {
	config.Callbacks.PeerConnAdded = append(config.Callbacks.PeerConnAdded, t.added)
	config.Callbacks.PeerConnClosed = t.closedPeer
	config.Callbacks.StatusUpdated = append(config.Callbacks.StatusUpdated, t.statusUpdated)
}

func (t *Tracker) added(peer *torrent.PeerConn) {
	now := time.Now()
	t.mu.Lock()
	t.peers[peer] = &state{connectedAt: now, lastProgress: now}
	t.mu.Unlock()
	t.handshakes.Add(1)
}

func (t *Tracker) closedPeer(peer *torrent.PeerConn) {
	t.mu.Lock()
	peerState, ok := t.peers[peer]
	delete(t.peers, peer)
	t.mu.Unlock()
	t.closed.Add(1)
	if !ok || isDead(peerState.usefulBytes) {
		t.dead.Add(1)
	}
}

func (t *Tracker) statusUpdated(event torrent.StatusUpdatedEvent) {
	switch event.Event {
	case torrent.TrackerAnnounceError, torrent.TrackerDisconnected:
		t.trackerErrors.Add(1)
	case torrent.TrackerAnnounceSuccessful, torrent.TrackerConnected:
		t.trackerSuccesses.Add(1)
	}
}

func (t *Tracker) Sample(peers []*torrent.PeerConn, now time.Time) Snapshot {
	type observation struct {
		peer        *torrent.PeerConn
		usefulBytes int64
	}
	observations := make([]observation, 0, len(peers))
	// Peer.Stats takes the torrent client's lock. Never hold our callback mutex
	// while calling it: PeerConnClosed can arrive with the client lock held and
	// then acquire our mutex, so the reverse order would deadlock.
	for _, peer := range peers {
		stats := peer.Stats()
		observations = append(observations, observation{
			peer:        peer,
			usefulBytes: stats.BytesReadUsefulData.Int64(),
		})
	}
	stalled := 0
	useful := 0
	t.mu.Lock()
	for _, observed := range observations {
		peerState, ok := t.peers[observed.peer]
		if !ok {
			peerState = &state{connectedAt: now, lastProgress: now}
			t.peers[observed.peer] = peerState
		}
		usefulBytes := observed.usefulBytes
		if usefulBytes > peerState.usefulBytes {
			peerState.usefulBytes = usefulBytes
			peerState.lastProgress = now
		}
		if usefulBytes > 0 {
			useful++
		}
		if isStalled(peerState.connectedAt, peerState.lastProgress, now, t.stalledAfter) {
			stalled++
		}
	}
	t.mu.Unlock()
	return Snapshot{
		Handshakes:       t.handshakes.Load(),
		Closed:           t.closed.Load(),
		Dead:             t.dead.Load(),
		Stalled:          stalled,
		Useful:           useful,
		TrackerErrors:    t.trackerErrors.Load(),
		TrackerSuccesses: t.trackerSuccesses.Load(),
	}
}

func isDead(usefulBytes int64) bool { return usefulBytes < deadUsefulBytes }

func isStalled(connectedAt, lastProgress, now time.Time, threshold time.Duration) bool {
	return now.Sub(connectedAt) >= threshold && now.Sub(lastProgress) >= threshold
}
