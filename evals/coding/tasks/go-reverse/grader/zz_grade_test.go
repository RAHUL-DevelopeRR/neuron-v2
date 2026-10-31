package textutil

import "testing"

func TestGradeReverse(t *testing.T) {
	cases := map[string]string{
		"":         "",
		"a":        "a",
		"abc":      "cba",
		"héllo":    "olléh",
		"日本語":      "語本日",
		"a😀b":      "b😀a",
		"racecar":  "racecar",
		"Go, 世界!": "!界世 ,oG",
	}
	for in, want := range cases {
		if got := Reverse(in); got != want {
			t.Errorf("Reverse(%q) = %q, want %q", in, got, want)
		}
	}
}
