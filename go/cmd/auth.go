package cmd

import (
	"bytes"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"strings"
	"time"

	"github.com/opencode-ai/opencode/internal/auth"
	"github.com/opencode-ai/opencode/internal/version"
	"github.com/spf13/cobra"
)

type gatewaySessionResponse struct {
	SessionToken string     `json:"session_token"`
	UserID       string     `json:"user_id"`
	Email        string     `json:"email"`
	Name         string     `json:"name"`
	ImageURL     string     `json:"image_url"`
	Plan         string     `json:"plan"`
	Provider     string     `json:"provider"`
	ExpiresAt    int64      `json:"expires_at"`
	TTLSeconds   int64      `json:"ttl_seconds"`
	Quota        auth.Quota `json:"quota"`
	Usage        auth.Usage `json:"usage"`
	Requests     int64      `json:"requests"`
	TokensUsed   int64      `json:"tokens_used"`
	Error        string     `json:"error"`
}

func newAuthCommand() *cobra.Command {
	cmd := &cobra.Command{
		Use:   "auth",
		Short: "Manage unified NeuronCLI authentication",
	}

	login := &cobra.Command{
		Use:   "login",
		Short: "Create and cache a zero-x.live gateway session",
		RunE: func(cmd *cobra.Command, args []string) error {
			gateway, _ := cmd.Flags().GetString("gateway")
			clerkToken, _ := cmd.Flags().GetString("clerk-token")
			userID, _ := cmd.Flags().GetString("user-id")
			email, _ := cmd.Flags().GetString("email")
			name, _ := cmd.Flags().GetString("name")
			plan, _ := cmd.Flags().GetString("plan")
			return runAuthLogin(gateway, clerkToken, userID, email, name, plan)
		},
	}
	login.Flags().String("gateway", defaultAuthGateway(), "Gateway URL")
	login.Flags().String("clerk-token", os.Getenv("CLERK_SESSION_TOKEN"), "Optional Clerk session JWT for unified account login")
	login.Flags().String("user-id", os.Getenv("NEURON_USER_ID"), "Optional user id for local/dev gateways")
	login.Flags().String("email", os.Getenv("NEURON_USER_EMAIL"), "Optional user email for local/dev gateways")
	login.Flags().String("name", os.Getenv("NEURON_USER_NAME"), "Optional display name for local/dev gateways")
	login.Flags().String("plan", os.Getenv("NEURON_USER_PLAN"), "Optional plan for local/dev gateways")

	status := &cobra.Command{
		Use:   "status",
		Short: "Show cached auth, plan, and usage state",
		RunE: func(cmd *cobra.Command, args []string) error {
			refresh, _ := cmd.Flags().GetBool("refresh")
			return runAuthStatus(refresh)
		},
	}
	status.Flags().Bool("refresh", true, "Refresh cached usage from the gateway when possible")

	logout := &cobra.Command{
		Use:   "logout",
		Short: "Remove cached NeuronCLI sessions",
		RunE: func(cmd *cobra.Command, args []string) error {
			if err := auth.DeleteSessions(); err != nil {
				return err
			}
			fmt.Println("Signed out. Cached NeuronCLI sessions removed.")
			return nil
		},
	}

	cmd.AddCommand(login, status, logout)
	return cmd
}

func runAuthLogin(gateway, clerkToken, userID, email, name, plan string) error {
	gateway = strings.TrimRight(strings.TrimSpace(gateway), "/")
	if gateway == "" {
		gateway = defaultAuthGateway()
	}
	body := map[string]string{
		"machine_fingerprint": machineFingerprint(),
		"version":             version.Version,
	}
	if userID != "" {
		body["user_id"] = userID
	}
	if email != "" {
		body["email"] = email
	}
	if name != "" {
		body["name"] = name
	}
	if plan != "" {
		body["plan"] = plan
	}
	payload, _ := json.Marshal(body)

	endpoint := gateway + "/auth/session"
	if clerkToken != "" {
		endpoint = gateway + "/auth/cli/session"
	}
	req, err := http.NewRequest(http.MethodPost, endpoint, bytes.NewReader(payload))
	if err != nil {
		return err
	}
	req.Header.Set("Content-Type", "application/json")
	if clerkToken != "" {
		req.Header.Set("Authorization", "Bearer "+clerkToken)
	}

	resp, err := (&http.Client{Timeout: 20 * time.Second}).Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	var session gatewaySessionResponse
	if err := json.NewDecoder(resp.Body).Decode(&session); err != nil {
		return err
	}
	if resp.StatusCode >= 400 || session.SessionToken == "" {
		if session.Error == "" {
			session.Error = resp.Status
		}
		return fmt.Errorf("auth login failed: %s", session.Error)
	}
	cache := cacheFromGatewayResponse(session, gateway)
	if err := auth.SaveSession(cache); err != nil {
		return err
	}

	fmt.Printf("Signed in to %s\n", gateway)
	fmt.Printf("User: %s\n", auth.DisplayName(cache))
	fmt.Printf("Plan: %s\n", cache.Plan)
	fmt.Printf("Session cache: %s\n", auth.SessionPath())
	return nil
}

func runAuthStatus(refresh bool) error {
	cache, path, ok := auth.LoadSession()
	if !ok {
		fmt.Println("Not signed in. Run: neuron auth login")
		return nil
	}
	if refresh {
		if refreshed, err := refreshSession(cache); err == nil {
			cache = refreshed
			_ = auth.SaveSession(cache)
		}
	}
	fmt.Printf("Signed in: yes\n")
	fmt.Printf("User: %s\n", auth.DisplayName(cache))
	fmt.Printf("Plan: %s\n", cache.Plan)
	fmt.Printf("Gateway: %s\n", cache.GatewayURL)
	fmt.Printf("Provider: %s\n", valueOr(cache.Provider, "gateway"))
	if cache.Quota.DailyLimit > 0 {
		fmt.Printf("Quota: %s used / %s daily (%s remaining)\n",
			formatCount(cache.Quota.Used),
			formatCount(cache.Quota.DailyLimit),
			formatCount(cache.Quota.Remaining),
		)
	}
	if cache.Usage.Requests > 0 || cache.Usage.TokensUsed > 0 {
		fmt.Printf("Usage: %d requests, %s tokens\n", cache.Usage.Requests, formatCount(cache.Usage.TokensUsed))
	}
	if cache.ExpiresAt > 0 {
		fmt.Printf("Expires: %s\n", time.Unix(cache.ExpiresAt, 0).Format(time.RFC3339))
	}
	fmt.Printf("Cache: %s\n", path)
	return nil
}

func refreshSession(cache auth.SessionCache) (auth.SessionCache, error) {
	if cache.GatewayURL == "" || cache.SessionToken == "" {
		return cache, fmt.Errorf("missing gateway or token")
	}
	req, err := http.NewRequest(http.MethodGet, strings.TrimRight(cache.GatewayURL, "/")+"/auth/session", nil)
	if err != nil {
		return cache, err
	}
	req.Header.Set("Authorization", "Bearer "+cache.SessionToken)
	resp, err := (&http.Client{Timeout: 10 * time.Second}).Do(req)
	if err != nil {
		return cache, err
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 400 {
		return cache, fmt.Errorf("status refresh failed: %s", resp.Status)
	}
	var session gatewaySessionResponse
	if err := json.NewDecoder(resp.Body).Decode(&session); err != nil {
		return cache, err
	}
	if session.UserID != "" {
		cache.UserID = session.UserID
	}
	if session.Email != "" {
		cache.Email = session.Email
	}
	if session.Name != "" {
		cache.Name = session.Name
	}
	if session.Plan != "" {
		cache.Plan = session.Plan
	}
	if session.Provider != "" {
		cache.Provider = session.Provider
	}
	if session.Quota.DailyLimit > 0 || session.Quota.Used > 0 || session.Quota.Remaining > 0 {
		cache.Quota = session.Quota
	}
	if session.Usage.Requests > 0 || session.Usage.TokensUsed > 0 {
		cache.Usage = session.Usage
	}
	if session.Requests > 0 {
		cache.Usage.Requests = session.Requests
	}
	if session.TokensUsed > 0 {
		cache.Usage.TokensUsed = session.TokensUsed
	}
	cache.Timestamp = time.Now().Unix()
	return cache, nil
}

func cacheFromGatewayResponse(session gatewaySessionResponse, gateway string) auth.SessionCache {
	expiresAt := session.ExpiresAt
	if expiresAt == 0 && session.TTLSeconds > 0 {
		expiresAt = time.Now().Add(time.Duration(session.TTLSeconds) * time.Second).Unix()
	}
	if session.Plan == "" {
		session.Plan = "free"
	}
	if session.Quota.DailyLimit == 0 {
		session.Quota.DailyLimit = auth.PlanLimit(session.Plan)
	}
	if session.Quota.Remaining == 0 && session.Quota.DailyLimit > 0 {
		session.Quota.Remaining = max(int64(0), session.Quota.DailyLimit-session.Quota.Used)
	}
	return auth.SessionCache{
		SessionToken: session.SessionToken,
		UserID:       session.UserID,
		Email:        session.Email,
		Name:         session.Name,
		ImageURL:     session.ImageURL,
		Plan:         session.Plan,
		GatewayURL:   gateway,
		Provider:     session.Provider,
		Timestamp:    time.Now().Unix(),
		ExpiresAt:    expiresAt,
		Quota:        session.Quota,
		Usage:        session.Usage,
	}
}

func defaultAuthGateway() string {
	if gateway := strings.TrimSpace(os.Getenv("NEURON_GATEWAY_URL")); gateway != "" {
		return strings.TrimRight(gateway, "/")
	}
	return "https://api.zero-x.live"
}

func machineFingerprint() string {
	hostname, _ := os.Hostname()
	user := os.Getenv("USER")
	if user == "" {
		user = os.Getenv("USERNAME")
	}
	fp := strings.Trim(user+"-"+hostname, "-")
	if fp == "" {
		return "neuron-cli"
	}
	return fp
}

func valueOr(value, fallback string) string {
	if strings.TrimSpace(value) == "" {
		return fallback
	}
	return value
}

func formatCount(v int64) string {
	switch {
	case v >= 1_000_000:
		return fmt.Sprintf("%.1fM", float64(v)/1_000_000)
	case v >= 1_000:
		return fmt.Sprintf("%.1fK", float64(v)/1_000)
	default:
		return fmt.Sprintf("%d", v)
	}
}
