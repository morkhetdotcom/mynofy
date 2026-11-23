//go:build !linux && !darwin

package mynofy

import (
	"errors"
	"net"
)

func peerUID(_ *net.UnixConn) (uint32, error) {
	return 0, errors.New("peer UID verification requires Linux or macOS")
}
