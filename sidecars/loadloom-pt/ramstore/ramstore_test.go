package ramstore

import (
	"bytes"
	"context"
	"errors"
	"io"
	"testing"

	"github.com/anacrolix/torrent/metainfo"
	"github.com/anacrolix/torrent/storage"
)

func testTorrent(t *testing.T, store *Store, pieceSize int64, pieces int) storage.TorrentImpl {
	t.Helper()
	info := &metainfo.Info{
		Name:        "ram-only-test",
		PieceLength: pieceSize,
		Length:      pieceSize * int64(pieces),
		Pieces:      make([]byte, pieces*metainfo.HashSize),
	}
	impl, err := store.OpenTorrent(context.Background(), info, metainfo.Hash{})
	if err != nil {
		t.Fatal(err)
	}
	return impl
}

func TestVerifiedPieceIsDiscardedButRemainsComplete(t *testing.T) {
	store, err := New(16 << 20)
	if err != nil {
		t.Fatal(err)
	}
	impl := testTorrent(t, store, 1<<20, 2)
	pieceInfo := metainfo.Info{
		Name:        "piece",
		PieceLength: 1 << 20,
		Length:      2 << 20,
		Pieces:      make([]byte, 2*metainfo.HashSize),
	}
	piece := impl.Piece(pieceInfo.Piece(0))
	payload := bytes.Repeat([]byte{0x5a}, 1<<20)
	if n, err := piece.WriteAt(payload, 0); err != nil || n != len(payload) {
		t.Fatalf("WriteAt = %d, %v", n, err)
	}
	readback := make([]byte, len(payload))
	if _, err := piece.ReadAt(readback, 0); err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(payload, readback) {
		t.Fatal("RAM data changed before verification")
	}
	if err := piece.MarkComplete(); err != nil {
		t.Fatal(err)
	}
	if got := piece.Completion(); !got.Ok || !got.Complete {
		t.Fatalf("unexpected completion: %+v", got)
	}
	if _, err := piece.ReadAt(readback, 0); !errors.Is(err, io.EOF) {
		t.Fatalf("completed payload should have been discarded, got %v", err)
	}
	stats := store.Stats()
	if stats.UsedBytes != 0 || stats.VerifiedBytes != 1<<20 || stats.GoodPieces != 1 {
		t.Fatalf("unexpected stats: %+v", stats)
	}
}

func TestCapacityRejectsAnotherIncompletePiece(t *testing.T) {
	store, err := New(16 << 20)
	if err != nil {
		t.Fatal(err)
	}
	impl := testTorrent(t, store, 12<<20, 2)
	info := &metainfo.Info{
		Name:        "piece",
		PieceLength: 12 << 20,
		Length:      24 << 20,
		Pieces:      make([]byte, 2*metainfo.HashSize),
	}
	if _, err := impl.Piece(info.Piece(0)).WriteAt([]byte{1}, 0); err != nil {
		t.Fatal(err)
	}
	if _, err := impl.Piece(info.Piece(1)).WriteAt([]byte{1}, 0); !errors.Is(err, ErrCapacity) {
		t.Fatalf("expected ErrCapacity, got %v", err)
	}
	if err := impl.Close(); err != nil {
		t.Fatal(err)
	}
	if got := store.Stats().UsedBytes; got != 0 {
		t.Fatalf("Close leaked %d bytes", got)
	}
}
