// +build windows

package main

import (
	"os"
	"syscall"
	"unsafe"
)

var (
	kernel32                       = syscall.NewLazyDLL("kernel32.dll")
	procGetConsoleMode             = kernel32.NewProc("GetConsoleMode")
	procSetConsoleMode             = kernel32.NewProc("SetConsoleMode")
)

const (
	enableVirtualTerminalProcessing = 0x0004
	enableProcessedOutput          = 0x0001
)

func init() {
	enableVTProcessing(os.Stdout)
	enableVTProcessing(os.Stderr)
}

func enableVTProcessing(f *os.File) {
	handle := syscall.Handle(f.Fd())
	var mode uint32
	r, _, _ := procGetConsoleMode.Call(uintptr(handle), uintptr(unsafe.Pointer(&mode)))
	if r == 0 {
		return // not a console
	}
	mode |= enableVirtualTerminalProcessing | enableProcessedOutput
	procSetConsoleMode.Call(uintptr(handle), uintptr(mode))
}
