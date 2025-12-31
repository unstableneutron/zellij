# Delta Optimization Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Reduce ZRP screen deltas from ~9KB to <1200 bytes so they fit within QUIC datagrams.

**Architecture:** Two-part optimization: (1) Use dirty_rows tracking instead of Arc::ptr_eq to limit row iteration to only changed rows, (2) Implement intra-row diffing to emit sparse CellRuns containing only changed columns instead of full 80-cell rows.

**Tech Stack:** Rust, prost (protobuf), zellij-remote-core crate

---

## Prerequisites

- QUIC datagrams and 0-RTT implementation complete ✅
- dirty_rows HashSet already exists in FrameStore (frame.rs line 117) ✅
- Client apply_delta already handles sparse CellRuns correctly (spike_client.rs lines 219-227) ✅

## Critical Design Decisions

### dirty_rows Caching Strategy

**Problem:** `get_render_update` is called per-client. If we call `take_dirty_rows()` per client, the first client clears the set and later clients get empty dirty_rows (missing updates).

**Solution:** Cache dirty_rows keyed by state_id in RemoteSession. When `advance_frame()` is called, capture dirty_rows from FrameStore and store with the new state_id. All clients for that state_id reuse the same dirty_rows.

### Row Patch Ordering

**Problem:** HashSet iteration is nondeterministic, making patches arrive in arbitrary order.

**Solution:** Sort candidate_rows before encoding patches (cheap - typically 1-3 dirty rows).

### Cell Deletion Handling

**Confirmed:** `Row::get_cell(col)` returns `None` only when col >= cells.len() (out of bounds), not for blank cells. Rows are always fully populated with explicit Cell values. The `(Some(_), None)` case only happens when columns shrink, which forces a snapshot anyway.

## Task Summary

| Task | Description | Expected Size Reduction |
|------|-------------|------------------------|
| 1 | Add dirty_rows cache to RemoteSession | N/A (infrastructure) |
| 2 | Thread dirty_rows through to DeltaEngine | N/A (API changes) |
| 3 | Update compute_delta to use dirty_rows | 24 rows → 1-3 rows |
| 4 | Implement baseline-aware encode_row_patch | 80 cells/row → 1-10 cells/row |
| 5 | Update session.rs to pass cached dirty_rows | N/A (wiring) |
| 6 | Update all call sites and tests | N/A (compatibility) |
| 7 | Add comprehensive unit tests | N/A (testing) |

**Expected final result:** Keystroke deltas ~50-200 bytes (fits datagrams!), prompt redraws ~300-400 bytes

---

## Task 1: Add dirty_rows Cache to RemoteSession

**Files:**
- Modify: `zellij-remote-core/src/session.rs`

**Step 1: Add dirty_rows cache field to RemoteSession**

Add a field to cache dirty_rows per state_id:

```rust
pub struct RemoteSession {
    pub frame_store: FrameStore,
    pub style_table: StyleTable,
    pub lease_manager: LeaseManager,
    pub input_receivers: HashMap<u64, InputReceiver>,
    pub rtt_estimator: RttEstimator,
    pub clients: HashMap<u64, ClientRenderState>,
    pub state_history: StateHistory,
    pub session_id: u64,
    token_expiry_ms: u64,
    max_clock_skew_ms: u64,
    token_secret: [u8; 32],
    /// Cached dirty_rows for current state_id (cleared on advance_frame)
    cached_dirty_rows: Option<(u64, HashSet<usize>)>,  // (state_id, dirty_rows)
}
```

**Step 2: Initialize in constructor**

Add `cached_dirty_rows: None` to both `new()` and `with_session_id()`.

**Step 3: Add method to get or capture dirty_rows**

```rust
    /// Get dirty_rows for current state, capturing from FrameStore on first call per state.
    pub fn get_dirty_rows_for_current_state(&mut self) -> &HashSet<usize> {
        let current_state_id = self.frame_store.current_state_id();
        
        // Check if we have cached dirty_rows for current state
        if let Some((cached_id, _)) = &self.cached_dirty_rows {
            if *cached_id == current_state_id {
                return &self.cached_dirty_rows.as_ref().unwrap().1;
            }
        }
        
        // Capture dirty_rows from FrameStore and cache
        let dirty = self.frame_store.take_dirty_rows();
        self.cached_dirty_rows = Some((current_state_id, dirty));
        &self.cached_dirty_rows.as_ref().unwrap().1
    }
    
    /// Clear dirty_rows cache (call when advancing to new state)
    pub fn clear_dirty_rows_cache(&mut self) {
        self.cached_dirty_rows = None;
    }
```

**Step 4: Run cargo check**

```bash
cargo check -p zellij-remote-core
```

Expected: Success (no call site changes yet)

---

## Task 2: Add dirty_rows Parameter to DeltaEngine::compute_delta

**Files:**
- Modify: `zellij-remote-core/src/delta.rs:1-61`

**Step 1: Add import and update signature**

```rust
use std::collections::HashSet;
// ... existing imports ...

impl DeltaEngine {
    pub fn compute_delta(
        baseline: &FrameData,
        current: &FrameData,
        style_table: &mut StyleTable,
        base_state_id: u64,
        current_state_id: u64,
        dirty_rows: Option<&HashSet<usize>>,  // NEW PARAMETER
    ) -> ScreenDelta {
```

**Step 2: Run cargo check**

```bash
cargo check -p zellij-remote-core
```

Expected: Compilation errors at call sites (will fix in Task 6)

---

## Task 3: Update compute_delta to Use dirty_rows

**Files:**
- Modify: `zellij-remote-core/src/delta.rs:19-45`

**Step 1: Replace Arc::ptr_eq iteration with dirty_rows-based iteration**

Replace the current row iteration logic (lines 22-36) with (note: sorting for deterministic order):

```rust
        let mut row_patches = Vec::new();
        let style_baseline = style_table.current_count();

        // Collect candidate rows: dirty_rows if provided, else fall back to all rows
        let mut candidate_rows: Vec<usize> = if let Some(dirty) = dirty_rows {
            // Only consider rows marked dirty (filtered to valid range)
            dirty.iter()
                .filter(|&idx| *idx < current.rows.len())
                .copied()
                .collect()
        } else {
            // Fallback: compare all overlapping rows using Arc::ptr_eq
            (0..std::cmp::min(baseline.rows.len(), current.rows.len()))
                .filter(|&idx| !Arc::ptr_eq(&baseline.rows[idx].0, &current.rows[idx].0))
                .collect()
        };
        
        // Sort for deterministic ordering (HashSet iteration is nondeterministic)
        candidate_rows.sort_unstable();

        // Process candidate rows
        for row_idx in candidate_rows {
            let baseline_row = baseline.rows.get(row_idx);
            let current_row = &current.rows[row_idx];
            
            if let Some(patch) = Self::encode_row_patch(row_idx, baseline_row, current_row) {
                row_patches.push(patch);
            }
        }

        // Handle new rows (current has more rows than baseline)
        if current.rows.len() > baseline.rows.len() {
            for row_idx in baseline.rows.len()..current.rows.len() {
                if let Some(patch) = Self::encode_row_patch(row_idx, None, &current.rows[row_idx]) {
                    row_patches.push(patch);
                }
            }
        }
```

**Step 2: Run cargo check**

```bash
cargo check -p zellij-remote-core
```

Expected: Error on encode_row_patch signature mismatch (will fix in Task 4)

---

## Task 4: Implement Baseline-Aware encode_row_patch with Intra-Row Diffing

**Files:**
- Modify: `zellij-remote-core/src/delta.rs:96-118`

**Step 1: Replace encode_row_patch with baseline-aware version**

Replace the existing `encode_row_patch` function with:

```rust
    /// Encode a row patch with sparse CellRuns containing only changed cells.
    /// Returns None if no cells changed (handles dirty false positives).
    fn encode_row_patch(row_idx: usize, baseline: Option<&Row>, current: &Row) -> Option<RowPatch> {
        let cols = current.cols();
        let mut runs: Vec<CellRun> = Vec::new();
        
        let mut col = 0;
        while col < cols {
            // Find start of changed region
            while col < cols && !Self::cell_changed(baseline, current, col) {
                col += 1;
            }
            
            if col >= cols {
                break;
            }
            
            // Found a changed cell - find the extent of the changed region
            let start_col = col;
            let mut codepoints = Vec::new();
            let mut widths = Vec::new();
            let mut style_ids = Vec::new();
            
            while col < cols && Self::cell_changed(baseline, current, col) {
                if let Some(cell) = current.get_cell(col) {
                    codepoints.push(cell.codepoint);
                    widths.push(cell.width as u32);
                    style_ids.push(cell.style_id as u32);
                }
                col += 1;
            }
            
            if !codepoints.is_empty() {
                runs.push(CellRun {
                    col_start: start_col as u32,
                    codepoints,
                    widths,
                    style_ids,
                });
            }
        }
        
        if runs.is_empty() {
            None
        } else {
            Some(RowPatch {
                row: row_idx as u32,
                runs,
            })
        }
    }

    /// Check if a cell has changed between baseline and current.
    /// Returns true if baseline is None (new row) or cell values differ.
    fn cell_changed(baseline: Option<&Row>, current: &Row, col: usize) -> bool {
        match baseline {
            None => true, // New row - all cells are "changed"
            Some(base_row) => {
                match (base_row.get_cell(col), current.get_cell(col)) {
                    (Some(base), Some(curr)) => {
                        base.codepoint != curr.codepoint
                            || base.width != curr.width
                            || base.style_id != curr.style_id
                    }
                    (None, Some(_)) => true, // New column
                    (Some(_), None) => true, // Deleted column
                    (None, None) => false,
                }
            }
        }
    }
```

**Step 2: Run cargo check**

```bash
cargo check -p zellij-remote-core
```

Expected: Success (or call site errors from Task 1)

---

## Task 5: Update session.rs to Pass Cached dirty_rows

**Files:**
- Modify: `zellij-remote-core/src/session.rs:160-163`

**Step 1: Update get_render_update to use cached dirty_rows**

In `zellij-remote-core/src/session.rs`, around line 160, change:

```rust
        } else if client_state.can_send() {
            // Get cached dirty_rows for current state (captures from FrameStore on first call)
            let dirty_rows = self.get_dirty_rows_for_current_state();
            let delta = client_state.prepare_delta(
                current_frame,
                current_state_id,
                &mut self.style_table,
                Some(dirty_rows),
            );
            delta.map(RenderUpdate::Delta)
        } else {
```

**Step 2: Run cargo check**

```bash
cargo check -p zellij-remote-core
```

Expected: Compilation errors at other call sites (will fix in Task 6)

---

## Task 6: Update All Call Sites and Tests

**Files:**
- Modify: `zellij-remote-core/src/client_state.rs:46-70`
- Modify: `zellij-remote-core/src/tests/delta_tests.rs` (all compute_delta calls)
- Modify: `zellij-remote-core/src/tests/session_tests.rs` (prepare_delta calls)
- Modify: `zellij-remote-core/src/tests/backpressure_tests.rs` (prepare_delta calls)

**Step 1: Update ClientRenderState::prepare_delta signature**

In `zellij-remote-core/src/client_state.rs`, change prepare_delta:

```rust
use std::collections::HashSet;

    pub fn prepare_delta(
        &mut self,
        current_frame: &FrameData,
        current_state_id: u64,
        style_table: &mut StyleTable,
        dirty_rows: Option<&HashSet<usize>>,  // NEW PARAMETER
    ) -> Option<ScreenDelta> {
        let baseline = self.acked_baseline.as_ref()?;

        if !self.render_window.can_send() {
            return None;
        }

        let delta = DeltaEngine::compute_delta(
            baseline,
            current_frame,
            style_table,
            self.acked_baseline_state_id,
            current_state_id,
            dirty_rows,  // PASS THROUGH
        );

        self.render_window.mark_sent(current_state_id);
        self.pending_frame = Some(current_frame.clone());
        self.pending_state_id = current_state_id;

        Some(delta)
    }
```

**Step 2: Update delta_tests.rs**

Add `None` as the last argument to all `compute_delta` calls (6 occurrences):

```rust
// Example change at line 25:
    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
        None,  // ADD THIS
    );
```

**Step 3: Update session_tests.rs**

Add `None` as the last argument to all `prepare_delta` calls (3 occurrences):

```rust
// Example at line 45:
    let delta1 = state.prepare_delta(&frame2, 2, &mut style_table, None);
```

**Step 4: Update backpressure_tests.rs**

Add `None` as the last argument to all `prepare_delta` calls (4 occurrences).

**Step 5: Run cargo check and tests**

```bash
cargo check -p zellij-remote-core
cargo test -p zellij-remote-core
```

Expected: All tests pass

**Step 6: Commit**

```bash
git add zellij-remote-core/src/
git commit -m "feat(delta): use dirty_rows and intra-row diffing for smaller deltas

- Add dirty_rows cache to RemoteSession (keyed by state_id)
- Add dirty_rows parameter to compute_delta and prepare_delta
- Replace Arc::ptr_eq with dirty_rows-based iteration
- Implement sparse CellRuns: only encode changed columns
- Sort candidate_rows for deterministic patch ordering

Expected: keystroke deltas ~50-200 bytes (fits QUIC datagrams)"
```

---

## Task 7: Add Comprehensive Unit Tests

**Files:**
- Modify: `zellij-remote-core/src/tests/delta_tests.rs`

**Step 1: Add test for single character change producing sparse run**

```rust
#[test]
fn test_intra_row_diff_single_char_change() {
    let mut store = FrameStore::new(80, 24);
    let baseline = store.snapshot();

    // Change only column 5
    store.update_row(0, |row| {
        row.set_cell(
            5,
            Cell {
                codepoint: 'X' as u32,
                width: 1,
                style_id: 0,
            },
        );
    });
    store.advance_state();
    let dirty = store.take_dirty_rows();

    let current = store.snapshot();
    let mut style_table = StyleTable::new();

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
        Some(&dirty),
    );

    // Should have exactly 1 row patch
    assert_eq!(delta.row_patches.len(), 1);
    assert_eq!(delta.row_patches[0].row, 0);
    
    // Should have exactly 1 run starting at col 5 with 1 cell
    assert_eq!(delta.row_patches[0].runs.len(), 1);
    assert_eq!(delta.row_patches[0].runs[0].col_start, 5);
    assert_eq!(delta.row_patches[0].runs[0].codepoints.len(), 1);
    assert_eq!(delta.row_patches[0].runs[0].codepoints[0], 'X' as u32);
}
```

**Step 2: Add test for non-contiguous changes producing multiple runs**

```rust
#[test]
fn test_intra_row_diff_non_contiguous_changes() {
    let mut store = FrameStore::new(80, 24);
    let baseline = store.snapshot();

    // Change columns 0 and 10 (non-contiguous)
    store.update_row(0, |row| {
        row.set_cell(0, Cell { codepoint: 'A' as u32, width: 1, style_id: 0 });
        row.set_cell(10, Cell { codepoint: 'B' as u32, width: 1, style_id: 0 });
    });
    store.advance_state();
    let dirty = store.take_dirty_rows();

    let current = store.snapshot();
    let mut style_table = StyleTable::new();

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
        Some(&dirty),
    );

    assert_eq!(delta.row_patches.len(), 1);
    
    // Should have 2 runs: one at col 0, one at col 10
    assert_eq!(delta.row_patches[0].runs.len(), 2);
    assert_eq!(delta.row_patches[0].runs[0].col_start, 0);
    assert_eq!(delta.row_patches[0].runs[0].codepoints[0], 'A' as u32);
    assert_eq!(delta.row_patches[0].runs[1].col_start, 10);
    assert_eq!(delta.row_patches[0].runs[1].codepoints[0], 'B' as u32);
}
```

**Step 3: Add test for dirty row with no actual changes (false positive)**

```rust
#[test]
fn test_dirty_row_false_positive_produces_no_patch() {
    let store = FrameStore::new(80, 24);
    let baseline = store.snapshot();
    let current = store.snapshot(); // Identical to baseline

    let mut style_table = StyleTable::new();
    
    // Manually mark row 5 as dirty even though nothing changed
    let mut dirty = std::collections::HashSet::new();
    dirty.insert(5);

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
        Some(&dirty),
    );

    // No actual changes, so no patches
    assert!(delta.row_patches.is_empty());
}
```

**Step 4: Add test for contiguous changes producing single run**

```rust
#[test]
fn test_intra_row_diff_contiguous_changes() {
    let mut store = FrameStore::new(80, 24);
    let baseline = store.snapshot();

    // Change columns 5-9 (contiguous span)
    store.update_row(0, |row| {
        for col in 5..10 {
            row.set_cell(col, Cell { codepoint: ('A' as u32) + (col - 5) as u32, width: 1, style_id: 0 });
        }
    });
    store.advance_state();
    let dirty = store.take_dirty_rows();

    let current = store.snapshot();
    let mut style_table = StyleTable::new();

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
        Some(&dirty),
    );

    assert_eq!(delta.row_patches.len(), 1);
    
    // Should have 1 run with 5 cells starting at col 5
    assert_eq!(delta.row_patches[0].runs.len(), 1);
    assert_eq!(delta.row_patches[0].runs[0].col_start, 5);
    assert_eq!(delta.row_patches[0].runs[0].codepoints.len(), 5);
}
```

**Step 5: Add test for style-only change producing minimal run**

```rust
#[test]
fn test_style_only_change_produces_run() {
    let mut store = FrameStore::new(80, 24);
    
    // Set initial cell with style 0
    store.update_row(0, |row| {
        row.set_cell(5, Cell { codepoint: 'X' as u32, width: 1, style_id: 0 });
    });
    store.advance_state();
    let baseline = store.snapshot();
    store.take_dirty_rows(); // Clear
    
    // Change only style (same codepoint/width)
    store.update_row(0, |row| {
        row.set_cell(5, Cell { codepoint: 'X' as u32, width: 1, style_id: 1 }); // Different style
    });
    store.advance_state();
    let dirty = store.take_dirty_rows();

    let current = store.snapshot();
    let mut style_table = StyleTable::new();

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
        Some(&dirty),
    );

    // Should detect style change
    assert_eq!(delta.row_patches.len(), 1);
    assert_eq!(delta.row_patches[0].runs.len(), 1);
    assert_eq!(delta.row_patches[0].runs[0].style_ids[0], 1);
}
```

**Step 6: Add test for multiple dirty rows with deterministic ordering**

```rust
#[test]
fn test_multiple_dirty_rows_ordered() {
    let mut store = FrameStore::new(80, 24);
    let baseline = store.snapshot();

    // Change rows 10, 5, 15 (out of order)
    store.update_row(10, |row| {
        row.set_cell(0, Cell { codepoint: 'A' as u32, width: 1, style_id: 0 });
    });
    store.update_row(5, |row| {
        row.set_cell(0, Cell { codepoint: 'B' as u32, width: 1, style_id: 0 });
    });
    store.update_row(15, |row| {
        row.set_cell(0, Cell { codepoint: 'C' as u32, width: 1, style_id: 0 });
    });
    store.advance_state();
    let dirty = store.take_dirty_rows();

    let current = store.snapshot();
    let mut style_table = StyleTable::new();

    let delta = DeltaEngine::compute_delta(
        &baseline.data,
        &current.data,
        &mut style_table,
        baseline.state_id,
        current.state_id,
        Some(&dirty),
    );

    // Should have 3 patches in sorted order
    assert_eq!(delta.row_patches.len(), 3);
    assert_eq!(delta.row_patches[0].row, 5);
    assert_eq!(delta.row_patches[1].row, 10);
    assert_eq!(delta.row_patches[2].row, 15);
}
```

**Step 7: Run tests**

```bash
cargo test -p zellij-remote-core -- delta_tests
```

Expected: All tests pass

**Step 8: Commit**

```bash
git add zellij-remote-core/src/tests/delta_tests.rs
git commit -m "test(delta): add comprehensive unit tests for intra-row diffing

- Single char change → sparse run at exact column
- Non-contiguous changes → multiple runs
- Dirty false positive → no patches emitted
- Contiguous changes → single run
- Style-only change detected
- Multiple dirty rows in deterministic order"
```

---

## Task 8: Verify End-to-End Tests Pass

**Files:** None (verification only)

**Step 1: Run all zellij-remote-core tests**

```bash
cargo test -p zellij-remote-core
```

Expected: All tests pass

**Step 2: Run E2E tests**

```bash
cd zellij-remote-tests && make test-all
```

Expected: All 21 tests pass

**Step 3: Run cargo x (full build + clippy + format)**

```bash
cargo x
```

Expected: Success

---

## Verification Checklist

- [x] dirty_rows cache added to RemoteSession (keyed by state_id)
- [x] dirty_rows parameter added to compute_delta
- [x] dirty_rows parameter added to prepare_delta  
- [x] Intra-row diffing produces sparse CellRuns
- [x] False positives (dirty but unchanged) produce no patches
- [x] Non-contiguous changes produce multiple runs
- [x] Row patches are in deterministic (sorted) order
- [x] Style-only changes detected
- [x] New rows not duplicated when dirty_rows provided (Oracle fix)
- [x] All existing tests pass (140 tests)
- [x] All new unit tests pass (7 added)
- [x] cargo build passes
- [ ] E2E tests pass (manual verification needed)

## Implementation Complete (2025-01-01)

All tasks implemented and Oracle review issues addressed.

## Post-Implementation Testing

After implementation, verify delta sizes with manual testing:

```bash
# On sjc3 (Tailscale):
ZELLIJ_REMOTE_ADDR=0.0.0.0:4433 ZELLIJ_REMOTE_TOKEN=test123 ./target/release/zellij

# On local machine:
SERVER_URL="https://100.69.153.168:4433" ZELLIJ_REMOTE_TOKEN=test123 \
    cargo run --release --example spike_client -p zellij-remote-bridge
```

Type "echo hello" and observe metrics:
- Before: deltas ~9KB (stream delivery)
- After: deltas ~50-200 bytes (datagram delivery)

Check for `Deltas via datagram: N` in metrics output.
