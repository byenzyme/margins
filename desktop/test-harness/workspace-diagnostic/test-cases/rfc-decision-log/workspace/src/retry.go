package main

func shouldRetry(err string) bool {
	if err == "rate-limited" {
		return true
	}
	return false
}
