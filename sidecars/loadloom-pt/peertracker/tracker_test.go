package peertracker

import (
	"testing"
	"time"
)

func TestDeadPeerClassificationRequiresUsefulPayload(t *testing.T) {
	if !isDead(0) || !isDead(deadUsefulBytes-1) {
		t.Fatal("short-lived non-useful peers must be classified as dead")
	}
	if isDead(deadUsefulBytes) {
		t.Fatal("a peer that delivered the minimum useful payload is not dead")
	}
}

func TestStallRequiresBothConnectionAgeAndNoProgress(t *testing.T) {
	now := time.Unix(100, 0)
	threshold := 15 * time.Second
	if !isStalled(now.Add(-20*time.Second), now.Add(-16*time.Second), now, threshold) {
		t.Fatal("old peer with no progress should be stalled")
	}
	if isStalled(now.Add(-20*time.Second), now.Add(-2*time.Second), now, threshold) {
		t.Fatal("recent progress must clear stalled state")
	}
	if isStalled(now.Add(-2*time.Second), now.Add(-2*time.Second), now, threshold) {
		t.Fatal("new connections need a grace period")
	}
}
