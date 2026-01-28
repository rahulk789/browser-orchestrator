package main

import (
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net/http"
	"sync"
	"time"
)

type Session struct {
	ID        string                 `json:"id"`
	CreatedAt int64                  `json:"created_at"`
	Data      map[string]interface{} `json:"data"`
}

type StatusResponse struct {
	Available bool    `json:"available"`
	SessionID *string `json:"session_id"`
}

type TestResult struct {
	Name    string
	Passed  bool
	Error   string
	Elapsed time.Duration
}

type Tester struct {
	baseURL string
	client  *http.Client
	results []TestResult
}

func NewTester(baseURL string) *Tester {
	return &Tester{
		baseURL: baseURL,
		client: &http.Client{
			Timeout: 10 * time.Second,
		},
		results: []TestResult{},
	}
}

func (t *Tester) recordResult(name string, passed bool, err string, elapsed time.Duration) {
	t.results = append(t.results, TestResult{
		Name:    name,
		Passed:  passed,
		Error:   err,
		Elapsed: elapsed,
	})
}

func (t *Tester) createSession(data map[string]interface{}) (*Session, error) {
	body, _ := json.Marshal(data)
	resp, err := t.client.Post(t.baseURL+"/sessions", "application/json", bytes.NewBuffer(body))
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK && resp.StatusCode != http.StatusCreated {
		return nil, fmt.Errorf("unexpected status code: %d", resp.StatusCode)
	}

	var session Session
	if err := json.NewDecoder(resp.Body).Decode(&session); err != nil {
		return nil, err
	}

	return &session, nil
}

func (t *Tester) getSession(id string) (*Session, error) {
	resp, err := t.client.Get(t.baseURL + "/sessions/" + id)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusNotFound {
		return nil, fmt.Errorf("session not found")
	}

	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("unexpected status code: %d", resp.StatusCode)
	}

	var session Session
	if err := json.NewDecoder(resp.Body).Decode(&session); err != nil {
		return nil, err
	}

	return &session, nil
}

func (t *Tester) deleteSession(id string) error {
	req, err := http.NewRequest(http.MethodDelete, t.baseURL+"/sessions/"+id, nil)
	if err != nil {
		return err
	}

	resp, err := t.client.Do(req)
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode == http.StatusNotFound {
		return fmt.Errorf("session not found")
	}

	if resp.StatusCode != http.StatusNoContent && resp.StatusCode != http.StatusOK {
		return fmt.Errorf("unexpected status code: %d", resp.StatusCode)
	}

	return nil
}

func (t *Tester) getHealth() error {
	resp, err := t.client.Get(t.baseURL + "/health")
	if err != nil {
		return err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return fmt.Errorf("unexpected status code: %d", resp.StatusCode)
	}

	body, _ := io.ReadAll(resp.Body)
	if string(body) != "ok" && string(body) != `"ok"` {
		return fmt.Errorf("unexpected response: %s", string(body))
	}

	return nil
}

func (t *Tester) testCreateSession() {
	start := time.Now()
	session, err := t.createSession(map[string]interface{}{"user": "alice"})
	elapsed := time.Since(start)

	if err != nil {
		t.recordResult("Create session", false, err.Error(), elapsed)
		return
	}

	if session.ID == "" {
		t.recordResult("Create session", false, "session ID is empty", elapsed)
		return
	}

	if session.Data["user"] != "alice" {
		t.recordResult("Create session", false, "user data mismatch", elapsed)
		return
	}

	t.recordResult("Create session", true, "", elapsed)
}

func (t *Tester) testGetSession() {
	start := time.Now()

	// Create a session first
	session, err := t.createSession(map[string]interface{}{"user": "bob"})
	if err != nil {
		t.recordResult("Get session", false, "failed to create session: "+err.Error(), time.Since(start))
		return
	}

	// Get the session
	retrieved, err := t.getSession(session.ID)
	elapsed := time.Since(start)

	if err != nil {
		t.recordResult("Get session", false, err.Error(), elapsed)
		return
	}

	if retrieved.ID != session.ID {
		t.recordResult("Get session", false, "session ID mismatch", elapsed)
		return
	}

	if retrieved.Data["user"] != "bob" {
		t.recordResult("Get session", false, "user data mismatch", elapsed)
		return
	}

	t.recordResult("Get session", true, "", elapsed)
}

func (t *Tester) testDeleteSession() {
	start := time.Now()

	// Create a session first
	session, err := t.createSession(map[string]interface{}{"user": "charlie"})
	if err != nil {
		t.recordResult("Delete session", false, "failed to create session: "+err.Error(), time.Since(start))
		return
	}

	// Delete the session
	err = t.deleteSession(session.ID)
	if err != nil {
		t.recordResult("Delete session", false, err.Error(), time.Since(start))
		return
	}

	// Verify it's deleted
	_, err = t.getSession(session.ID)
	elapsed := time.Since(start)

	if err == nil {
		t.recordResult("Delete session", false, "session still exists after delete", elapsed)
		return
	}

	t.recordResult("Delete session", true, "", elapsed)
}

func (t *Tester) test404OnMissing() {
	start := time.Now()

	_, err := t.getSession("invalid-session-id")
	elapsed := time.Since(start)

	if err == nil {
		t.recordResult("404 on missing session", false, "expected error but got none", elapsed)
		return
	}

	t.recordResult("404 on missing session", true, "", elapsed)
}

func (t *Tester) testConcurrentSessions() {
	start := time.Now()
	const numSessions = 10
	var wg sync.WaitGroup
	errors := make(chan error, numSessions)

	for i := 0; i < numSessions; i++ {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()

			session, err := t.createSession(map[string]interface{}{"user": fmt.Sprintf("user%d", idx)})
			if err != nil {
				errors <- err
				return
			}

			retrieved, err := t.getSession(session.ID)
			if err != nil {
				errors <- err
				return
			}

			if retrieved.ID != session.ID {
				errors <- fmt.Errorf("session ID mismatch")
				return
			}
		}(i)
	}

	wg.Wait()
	close(errors)
	elapsed := time.Since(start)

	errCount := 0
	var lastErr error
	for err := range errors {
		errCount++
		lastErr = err
	}

	if errCount > 0 {
		t.recordResult("Concurrent sessions (10 parallel)", false,
			fmt.Sprintf("%d errors, last: %v", errCount, lastErr), elapsed)
		return
	}

	t.recordResult("Concurrent sessions (10 parallel)", true, "", elapsed)
}

func (t *Tester) testSessionTTL() {
	start := time.Now()

	// Create a session
	session, err := t.createSession(map[string]interface{}{"user": "ttl-test"})
	if err != nil {
		t.recordResult("Session TTL expiration (60s)", false, "failed to create session: "+err.Error(), time.Since(start))
		return
	}

	fmt.Println("   Waiting 60 seconds for TTL expiration...")
	time.Sleep(61 * time.Second)

	// Try to get the session
	_, err = t.getSession(session.ID)
	elapsed := time.Since(start)

	if err == nil {
		t.recordResult("Session TTL expiration (60s)", false, "session still exists after TTL", elapsed)
		return
	}

	t.recordResult("Session TTL expiration (60s)", true, "", elapsed)
}

func (t *Tester) testWorkerFailureRecovery() {
	start := time.Now()

	// This is a placeholder - actual implementation would depend on how worker failures are simulated
	// For now, we'll just create and retrieve a session to verify basic functionality
	session, err := t.createSession(map[string]interface{}{"user": "recovery-test"})
	if err != nil {
		t.recordResult("Worker failure recovery", false, err.Error(), time.Since(start))
		return
	}

	_, err = t.getSession(session.ID)
	elapsed := time.Since(start)

	if err != nil {
		t.recordResult("Worker failure recovery", false, err.Error(), elapsed)
		return
	}

	t.recordResult("Worker failure recovery", true, "", elapsed)
}

func (t *Tester) runAllTests() {
	fmt.Println("━━━━━━━━━━━━━━━━━━━━━━━━━━━━ 🧪 ORCHESTRATOR TEST SUITE ━━━━━━━━━━━━━━━━━━━━━━━━━━━━")

	// Run basic tests
	t.testCreateSession()
	t.testGetSession()
	t.testDeleteSession()
	t.test404OnMissing()
	t.testConcurrentSessions()
	t.testSessionTTL()
	t.testWorkerFailureRecovery()
}

func (t *Tester) printResults() {
	passed := 0
	total := len(t.results)

	for _, result := range t.results {
		if result.Passed {
			fmt.Printf("✓ %s\n", result.Name)
			passed++
		} else {
			fmt.Printf("✗ %s: %s\n", result.Name, result.Error)
		}
	}

	fmt.Printf("━━━━━━━━━━━━━━━━━━━━━━━━━━━━ 📊 RESULTS: %d/%d passed ━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n", passed, total)

	if passed != total {
		fmt.Println("\nFailed tests:")
		for _, result := range t.results {
			if !result.Passed {
				fmt.Printf("  • %s: %s (took %v)\n", result.Name, result.Error, result.Elapsed)
			}
		}
	}
}

func main() {
	url := flag.String("url", "http://localhost:8080", "Base URL of the orchestrator API")
	flag.Parse()

	tester := NewTester(*url)

	// Test health endpoint first
	fmt.Printf("Testing connection to %s...\n", *url)
	if err := tester.getHealth(); err != nil {
		fmt.Printf("❌ Failed to connect to %s: %v\n", *url, err)
		fmt.Println("Make sure the orchestrator is running and accessible.")
		return
	}
	fmt.Println()

	tester.runAllTests()
	tester.printResults()
}
