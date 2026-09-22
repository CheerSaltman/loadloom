// Package ramstore implements a bounded BitTorrent piece store that never
// writes payload data to disk. Incomplete pieces live in RAM only long enough
// for the torrent engine to hash them. MarkComplete releases their bytes while
// retaining the completion bit, which lets the download continue without
// keeping an ISO-sized image in memory.
package ramstore

import (
	"context"
	"errors"
	"fmt"
	"io"
	"sync"
	"sync/atomic"

	"github.com/anacrolix/torrent/metainfo"
	"github.com/anacrolix/torrent/storage"
)

var ErrCapacity = errors.New("RAM piece buffer capacity exceeded")

type Stats struct {
	UsedBytes      int64
	PeakBytes      int64
	VerifiedBytes  int64
	DiscardedBytes int64
	GoodPieces     int64
	BadPieces      int64
}

// Store is safe for concurrent piece reads and writes.
type Store struct {
	maxBytes       int64
	usedBytes      atomic.Int64
	peakBytes      atomic.Int64
	verifiedBytes  atomic.Int64
	discardedBytes atomic.Int64
	goodPieces     atomic.Int64
	badPieces      atomic.Int64
}

func New(maxBytes int64) (*Store, error) {
	if maxBytes < 16<<20 {
		return nil, fmt.Errorf("RAM limit must be at least 16 MiB")
	}
	return &Store{maxBytes: maxBytes}, nil
}

func (s *Store) MaxBytes() int64 { return s.maxBytes }

func (s *Store) Stats() Stats {
	return Stats{
		UsedBytes:      s.usedBytes.Load(),
		PeakBytes:      s.peakBytes.Load(),
		VerifiedBytes:  s.verifiedBytes.Load(),
		DiscardedBytes: s.discardedBytes.Load(),
		GoodPieces:     s.goodPieces.Load(),
		BadPieces:      s.badPieces.Load(),
	}
}

func (s *Store) OpenTorrent(
	_ context.Context,
	info *metainfo.Info,
	_ metainfo.Hash,
) (storage.TorrentImpl, error) {
	if info == nil || info.NumPieces() <= 0 {
		return storage.TorrentImpl{}, errors.New("torrent has no pieces")
	}
	t := &torrentStore{
		owner:  s,
		pieces: make([]*pieceState, info.NumPieces()),
	}
	for index := range t.pieces {
		piece := info.Piece(index)
		t.pieces[index] = &pieceState{owner: s, length: piece.Length()}
	}
	capacity := func() (int64, bool) { return s.maxBytes, true }
	return storage.TorrentImpl{
		Piece: func(piece metainfo.Piece) storage.PieceImpl {
			return t.piece(piece.Index())
		},
		Close:    t.close,
		Capacity: &capacity,
	}, nil
}

type torrentStore struct {
	owner     *Store
	pieces    []*pieceState
	closeOnce sync.Once
}

func (t *torrentStore) piece(index int) storage.PieceImpl {
	if index < 0 || index >= len(t.pieces) {
		return &invalidPiece{err: fmt.Errorf("piece index %d out of range", index)}
	}
	return t.pieces[index]
}

func (t *torrentStore) close() error {
	t.closeOnce.Do(func() {
		for _, piece := range t.pieces {
			piece.release(false)
		}
	})
	return nil
}

type pieceState struct {
	owner    *Store
	length   int64
	mu       sync.RWMutex
	data     []byte
	complete bool
}

func (p *pieceState) ensureBuffer() error {
	if p.data != nil {
		return nil
	}
	if !p.owner.reserve(p.length) {
		return ErrCapacity
	}
	p.data = make([]byte, int(p.length))
	return nil
}

func (p *pieceState) WriteAt(data []byte, offset int64) (int, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.complete {
		return 0, errors.New("cannot write a completed piece")
	}
	if offset < 0 || offset > p.length || int64(len(data)) > p.length-offset {
		return 0, io.ErrShortWrite
	}
	if err := p.ensureBuffer(); err != nil {
		return 0, err
	}
	return copy(p.data[int(offset):], data), nil
}

func (p *pieceState) ReadAt(dst []byte, offset int64) (int, error) {
	p.mu.RLock()
	defer p.mu.RUnlock()
	if offset < 0 || offset >= p.length || p.data == nil {
		return 0, io.EOF
	}
	n := copy(dst, p.data[int(offset):])
	if n != len(dst) {
		return n, io.EOF
	}
	return n, nil
}

func (p *pieceState) Completion() storage.Completion {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return storage.Completion{Complete: p.complete, Ok: true}
}

func (p *pieceState) MarkComplete() error {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.complete {
		return nil
	}
	if p.data == nil {
		return errors.New("cannot complete a piece without buffered data")
	}
	p.complete = true
	p.owner.verifiedBytes.Add(p.length)
	p.owner.goodPieces.Add(1)
	p.owner.discardedBytes.Add(int64(len(p.data)))
	p.owner.usedBytes.Add(-int64(len(p.data)))
	p.data = nil
	return nil
}

func (p *pieceState) MarkNotComplete() error {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.data != nil {
		p.owner.discardedBytes.Add(int64(len(p.data)))
		p.owner.usedBytes.Add(-int64(len(p.data)))
		p.data = nil
	}
	p.complete = false
	p.owner.badPieces.Add(1)
	return nil
}

func (p *pieceState) release(countDiscard bool) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.data == nil {
		return
	}
	if countDiscard {
		p.owner.discardedBytes.Add(int64(len(p.data)))
	}
	p.owner.usedBytes.Add(-int64(len(p.data)))
	p.data = nil
}

func (s *Store) reserve(size int64) bool {
	for {
		used := s.usedBytes.Load()
		if size < 0 || used > s.maxBytes-size {
			return false
		}
		if s.usedBytes.CompareAndSwap(used, used+size) {
			peak := used + size
			for current := s.peakBytes.Load(); peak > current; current = s.peakBytes.Load() {
				if s.peakBytes.CompareAndSwap(current, peak) {
					break
				}
			}
			return true
		}
	}
}

type invalidPiece struct{ err error }

func (p *invalidPiece) ReadAt([]byte, int64) (int, error)  { return 0, p.err }
func (p *invalidPiece) WriteAt([]byte, int64) (int, error) { return 0, p.err }
func (p *invalidPiece) MarkComplete() error                { return p.err }
func (p *invalidPiece) MarkNotComplete() error             { return p.err }
func (p *invalidPiece) Completion() storage.Completion {
	return storage.Completion{Err: p.err}
}
