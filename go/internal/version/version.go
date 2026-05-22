package version

import "runtime/debug"

// Build-time parameters set via -ldflags
var Version = "6.2.5"

func init() {
	info, ok := debug.ReadBuildInfo()
	if !ok {
		return
	}
	mainVersion := info.Main.Version
	if mainVersion == "" || mainVersion == "(devel)" {
		return
	}
	Version = mainVersion
}
