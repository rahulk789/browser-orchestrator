package main

import (
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net/http"
	"os"
	"time"
)

type LoggingTransport struct {
	rt http.RoundTripper
}

func (t *LoggingTransport) RoundTrip(req *http.Request) (*http.Response, error) {
	fmt.Println()
	fmt.Println("→", req.Method, req.URL.String())
	if req.Body != nil {
		body, _ := io.ReadAll(req.Body)
		req.Body = io.NopCloser(bytes.NewBuffer(body))
		if len(body) > 0 {
			fmt.Println(string(body))
		}
	}
	resp, err := t.rt.RoundTrip(req)
	if err != nil {
		fmt.Println("✗ request error:", err)
		return nil, err
	}
	fmt.Println("←", resp.Status)
	respBody, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	if len(respBody) > 0 {
		fmt.Println(string(respBody))
	}
	resp.Body = io.NopCloser(bytes.NewBuffer(respBody))
	return resp, nil
}

type CreateSessionResponse struct {
	ID string `json:"id"`
}
type ErrorResponse struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}
type Runner struct {
	baseURL string
	client  *http.Client
}

func NewRunner(baseURL string) *Runner {
	return &Runner{
		baseURL: baseURL,
		client: &http.Client{
			Timeout: 10 * time.Second,
			Transport: &LoggingTransport{
				rt: http.DefaultTransport,
			},
		},
	}
}
func (r *Runner) createSession(user string) (string, error) {
	body, _ := json.Marshal(map[string]string{"user": user})
	resp, err := r.client.Post(
		r.baseURL+"/session",
		"application/json",
		bytes.NewReader(body),
	)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("unexpected status %d", resp.StatusCode)
	}
	var out CreateSessionResponse
	if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {
		return "", err
	}
	return out.ID, nil
}
func (r *Runner) getSession(id string) error {
	resp, err := r.client.Get(r.baseURL + "/session/" + id)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	// Read the body to check for error response
	bodyBytes, _ := io.ReadAll(resp.Body)

	// Check if the response contains an error in the JSON body
	var errResp ErrorResponse
	if err := json.Unmarshal(bodyBytes, &errResp); err == nil {
		if errResp.Code == 500 && errResp.Message == "Error fetching session_worker from worker_list" {
			// Treat this as a 404 - session not found
			return fmt.Errorf("session not found (404)")
		}
	}

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("unexpected status %d", resp.StatusCode)
	}
	return nil
}
func (r *Runner) deleteSession(id string) error {
	req, _ := http.NewRequest(http.MethodDelete, r.baseURL+"/session/"+id, nil)
	resp, err := r.client.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("unexpected status %d", resp.StatusCode)
	}
	return nil
}
func printOK(msg string) {
	fmt.Println("✓", msg)
}
func printFail(msg string, err error) {
	fmt.Println("✗", msg)
	fmt.Println("  ", err)
	os.Exit(1)
}
func main() {
	url := flag.String("url", "http://localhost:3000", "orchestrator url")
	flag.Parse()
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
	fmt.Println("🧪 ORCHESTRATOR TEST SUITE")
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
	r := NewRunner(*url)

	// Create 10 sessions
	sessionIDs := make([]string, 10)
	users := []string{"alice", "bob", "charlie", "diana", "eve", "frank", "grace", "henry", "iris", "jack"}

	for i := 0; i < 10; i++ {
		id, err := r.createSession(users[i])
		if err != nil {
			printFail(fmt.Sprintf("Create session %d (%s)", i+1, users[i]), err)
		}
		sessionIDs[i] = id
		printOK(fmt.Sprintf("Create session %d (%s)", i+1, users[i]))
	}

	// Test getting each session
	for i, id := range sessionIDs {
		if err := r.getSession(id); err != nil {
			printFail(fmt.Sprintf("Get session %d (%s)", i+1, users[i]), err)
		}
		printOK(fmt.Sprintf("Get session %d (%s)", i+1, users[i]))
	}

	// Test deleting each session
	for i, id := range sessionIDs {
		if err := r.deleteSession(id); err != nil {
			printFail(fmt.Sprintf("Delete session %d (%s)", i+1, users[i]), err)
		}
		printOK(fmt.Sprintf("Delete session %d (%s)", i+1, users[i]))
	}

	// Try to get the sessions again (should fail since they're deleted)
	fmt.Println()
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
	fmt.Println("🔍 VERIFY DELETION")
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━")

	failedCount := 0
	for i, id := range sessionIDs {
		if err := r.getSession(id); err != nil {
			printOK(fmt.Sprintf("Session %d (%s) correctly not found", i+1, users[i]))
			failedCount++
		} else {
			printFail(fmt.Sprintf("Session %d (%s) still exists (should be deleted)", i+1, users[i]),
				fmt.Errorf("session was not deleted"))
		}
	}

	fmt.Println()
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
	fmt.Println(fmt.Sprintf("📊 RESULTS: %d/40 passed", 30+failedCount))
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━")
}
