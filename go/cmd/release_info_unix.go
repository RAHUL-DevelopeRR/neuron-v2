//go:build !windows

package cmd

import "os/exec"

func init() {
	execLookPath = exec.LookPath
}
