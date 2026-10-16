package main

import (
	"os"
	"path/filepath"
	"sync"
)

const sizerCacheDepth = 4

type sizer struct {
	mu      sync.Mutex
	cond    *sync.Cond
	sizes   map[string]int64
	pending map[string]bool
	stack   []string
	updates chan struct{}
	closed  bool
}

func newSizer(workers int) *sizer {
	s := &sizer{sizes: map[string]int64{}, pending: map[string]bool{}, updates: make(chan struct{}, 1)}
	s.cond = sync.NewCond(&s.mu)
	for range workers {
		go s.work()
	}
	return s
}

func (s *sizer) request(path string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if _, done := s.sizes[path]; done || s.pending[path] || s.closed {
		return
	}
	s.pending[path] = true
	s.stack = append(s.stack, path)
	s.cond.Signal()
}

func (s *sizer) get(path string) (int64, bool) {
	s.mu.Lock()
	defer s.mu.Unlock()
	n, ok := s.sizes[path]
	return n, ok
}

func (s *sizer) close() {
	s.mu.Lock()
	s.closed = true
	s.stack = nil
	s.cond.Broadcast()
	s.mu.Unlock()
}

func (s *sizer) work() {
	for {
		s.mu.Lock()
		for len(s.stack) == 0 && !s.closed {
			s.cond.Wait()
		}
		if s.closed {
			s.mu.Unlock()
			return
		}
		p := s.stack[len(s.stack)-1]
		s.stack = s.stack[:len(s.stack)-1]
		s.mu.Unlock()
		s.walk(p, 0)
		s.mu.Lock()
		delete(s.pending, p)
		s.mu.Unlock()
		select {
		case s.updates <- struct{}{}:
		default:
		}
	}
}

func (s *sizer) walk(dir string, depth int) int64 {
	if n, ok := s.get(dir); ok {
		return n
	}
	s.mu.Lock()
	closed := s.closed
	s.mu.Unlock()
	if closed {
		return 0
	}
	entries, _ := os.ReadDir(dir)
	var total int64
	for _, e := range entries {
		if e.IsDir() {
			total += s.walk(filepath.Join(dir, e.Name()), depth+1)
			continue
		}
		if info, err := e.Info(); err == nil {
			total += info.Size()
		}
	}
	if depth <= sizerCacheDepth {
		s.mu.Lock()
		s.sizes[dir] = total
		s.mu.Unlock()
	}
	return total
}
