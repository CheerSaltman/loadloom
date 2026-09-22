package main

import (
	"testing"
	"time"
)

func TestPressureStatePrioritizesHardFailures(t *testing.T) {
	level, _ := pressureState(time.Minute, 20, 20, 20, 20, time.Minute, 1, 10, 1)
	if level != "critical" {
		t.Fatalf("storage error must be critical, got %q", level)
	}
}

func TestPressureStateDetectsDeadAndStalledPeers(t *testing.T) {
	level, _ := pressureState(30*time.Second, 10, 6, 0, 0, time.Second, 1, 10, 0)
	if level != "warning" {
		t.Fatalf("stalled majority should warn, got %q", level)
	}
	level, _ = pressureState(30*time.Second, 10, 0, 7, 10, time.Second, 1, 10, 0)
	if level != "warning" {
		t.Fatalf("high dead-peer ratio should warn, got %q", level)
	}
}

func TestPressureStateAllowsWarmup(t *testing.T) {
	level, _ := pressureState(10*time.Second, 0, 0, 0, 0, 10*time.Second, 0, 100, 0)
	if level != "normal" {
		t.Fatalf("discovery warmup should stay normal, got %q", level)
	}
}
