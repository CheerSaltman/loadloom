package main

import (
	"fmt"
	"time"
)

func pressureState(
	elapsed time.Duration,
	activePeers int,
	stalledPeers int,
	deadPeers int64,
	closedPeers int64,
	lastProgressAge time.Duration,
	ramUsed int64,
	ramLimit int64,
	storageErrors int64,
) (string, string) {
	if storageErrors > 0 {
		return "critical", fmt.Sprintf("RAM piece storage reported %d capacity/write errors", storageErrors)
	}
	if elapsed >= 60*time.Second && lastProgressAge >= 60*time.Second {
		return "critical", "no useful piece data received for 60 seconds"
	}
	if elapsed >= 20*time.Second && activePeers == 0 {
		return "warning", "no active peers; tracker/DHT discovery or upstream NAT may be limiting the run"
	}
	if activePeers > 0 && stalledPeers*2 >= activePeers {
		return "warning", fmt.Sprintf("%d of %d active peers are stalled", stalledPeers, activePeers)
	}
	if closedPeers >= 10 && deadPeers*100/closedPeers >= 60 {
		return "warning", fmt.Sprintf("dead-peer ratio is %d%%", deadPeers*100/closedPeers)
	}
	if ramLimit > 0 && ramUsed*100/ramLimit >= 85 {
		return "warning", "RAM piece buffer is above 85%"
	}
	return "normal", "swarm pressure is normal"
}
