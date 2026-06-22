package auth

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

const (
	defaultGatewayURL = "https://api.zero-x.live"
	defaultPlan       = "free"
)

type Quota struct {
	DailyLimit int64 `json:"daily_limit,omitempty"`
	Used       int64 `json:"used,omitempty"`
	Remaining  int64 `json:"remaining,omitempty"`
}

type Usage struct {
	Requests   int64 `json:"requests,omitempty"`
	TokensUsed int64 `json:"tokens_used,omitempty"`
}

type SessionCache struct {
	SessionToken string `json:"session_token"`
	Token        string `json:"token,omitempty"`
	UserID       string `json:"user_id,omitempty"`
	Email        string `json:"email,omitempty"`
	Name         string `json:"name,omitempty"`
	ImageURL     string `json:"image_url,omitempty"`
	Plan         string `json:"plan,omitempty"`
	GatewayURL   string `json:"gateway_url,omitempty"`
	Provider     string `json:"provider,omitempty"`
	Timestamp    int64  `json:"timestamp"`
	ExpiresAt    int64  `json:"expires_at,omitempty"`
	Quota        Quota  `json:"quota,omitempty"`
	Usage        Usage  `json:"usage,omitempty"`
}

func SessionPath() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".neuroncli", "session.json")
}

func LegacySessionPath() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".neuron", "session_cache.json")
}

func LoadSession() (SessionCache, string, bool) {
	for _, path := range []string{SessionPath(), LegacySessionPath()} {
		data, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		var cache SessionCache
		if err := json.Unmarshal(data, &cache); err != nil {
			continue
		}
		if cache.SessionToken == "" {
			cache.SessionToken = cache.Token
		}
		if cache.SessionToken == "" {
			continue
		}
		if cache.GatewayURL == "" {
			cache.GatewayURL = defaultGatewayURL
		}
		if cache.Plan == "" {
			cache.Plan = defaultPlan
		}
		if cache.ExpiresAt > 0 && time.Now().Unix() > cache.ExpiresAt {
			continue
		}
		if cache.ExpiresAt == 0 && cache.Timestamp > 0 && time.Now().Unix()-cache.Timestamp > 12*60*60 {
			continue
		}
		return cache, path, true
	}
	return SessionCache{}, "", false
}

func SaveSession(cache SessionCache) error {
	if cache.SessionToken == "" {
		cache.SessionToken = cache.Token
	}
	if cache.SessionToken == "" {
		return fmt.Errorf("session token is required")
	}
	if cache.Timestamp == 0 {
		cache.Timestamp = time.Now().Unix()
	}
	if cache.GatewayURL == "" {
		cache.GatewayURL = defaultGatewayURL
	}
	if cache.Plan == "" {
		cache.Plan = defaultPlan
	}
	path := SessionPath()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	data, err := json.MarshalIndent(cache, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(path, data, 0o600)
}

func DeleteSessions() error {
	var errs []string
	for _, path := range []string{SessionPath(), LegacySessionPath()} {
		if err := os.Remove(path); err != nil && !os.IsNotExist(err) {
			errs = append(errs, err.Error())
		}
	}
	if len(errs) > 0 {
		return fmt.Errorf("%s", strings.Join(errs, "; "))
	}
	return nil
}

func Token() string {
	cache, _, ok := LoadSession()
	if !ok {
		return ""
	}
	return cache.SessionToken
}

func PlanLimit(plan string) int64 {
	switch strings.ToLower(strings.TrimSpace(plan)) {
	case "ultrawork", "ultra", "enterprise":
		return 5_000_000
	case "pro":
		return 1_000_000
	default:
		return 44_000
	}
}

func DisplayName(cache SessionCache) string {
	switch {
	case cache.Name != "":
		return cache.Name
	case cache.Email != "":
		return cache.Email
	case cache.UserID != "":
		return cache.UserID
	default:
		return "anonymous"
	}
}
