# Task: Enable Windows Parity Tests in GitHub Actions

**Date**: January 17, 2026
**Priority**: MEDIUM
**Estimated Effort**: 4-6 hours
**Status**: TODO

---

## Problem Statement

Windows parity tests are **disabled** in `.github/workflows/parity-unified.yml` (lines 78-120).

**Reason**: "Awaiting headless parity-capture port"

Only macOS currently has functional headless parity testing in GitHub Actions. Windows has all the infrastructure (parity-capture binary, Python scripts, baseline images) but cannot run headless in CI.

## Current State

### ✅ What Windows Has:
- `parity-capture` binary (builds successfully)
- Complete parity scripts:
  - `scripts/parity_swarm.py` - Parallel test runner
  - `scripts/parity_gate.py` - Pass/fail gate
  - `scripts/parity_lib.py` - Common utilities
  - `scripts/parity_baseline.py` - Baseline management
  - `scripts/parity_compare.py` - Image comparison
  - `scripts/parity_summary.py` - Report generation
- Baseline images in `baselines/`
- Badge placeholder: `badges/parity-windows.svg`

### ❌ What's Missing:
- **Headless rendering support** in `parity-capture` for Windows GitHub Actions runners
- Windows runners need offscreen/virtual display capability
- Currently requires visible desktop window (not available in CI)

## macOS Reference (Working Solution)

macOS parity tests work in GitHub Actions (lines 29-76):
- Build `parity-capture` in release mode
- Run `parity_swarm.py` with 4 shards (parallel execution)
- Upload results as artifacts
- Aggregate across all shards

## Solution Approaches

### Option 1: Software Rendering (Recommended)
Enable wgpu software rendering for headless mode.

**Changes needed:**
1. **Modify `parity-capture/src/main.rs`**:
   - Add `--headless` flag
   - When headless, force wgpu software backend:
     ```rust
     let backends = if headless {
         wgpu::Backends::DX12 // or Vulkan with software
     } else {
         wgpu::Backends::all()
     };
     ```
   - Create offscreen texture target instead of window surface

2. **Update RustKit compositor**:
   - Support `headless` mode in `rustkit-compositor`
   - Render to texture instead of screen
   - Copy texture to CPU for screenshot

3. **Test locally**:
   - Verify `parity-capture --headless` works on Windows without display
   - Compare output images to baseline

### Option 2: Virtual Display (Xvfb equivalent)
Windows doesn't have native Xvfb, but alternatives exist:
- **VcXsrv** - X Server for Windows
- **Mesa llvmpipe** - Software OpenGL renderer
- **Windows Virtual Display Driver**

**Pros**: Works with existing windowed code
**Cons**: Requires external dependencies, complex CI setup

### Option 3: Remote Desktop Services (RDP)
Use Windows Remote Desktop to create virtual session.

**Pros**: Native Windows solution
**Cons**: Complex setup, requires RDP configuration in CI

## Recommended Implementation (Option 1)

### Step 1: Add Headless Flag to parity-capture

**File**: `hiwave-windows/crates/parity-capture/src/main.rs`

```rust
#[derive(Parser)]
struct Args {
    // ... existing args ...

    /// Run in headless mode (no window, offscreen rendering)
    #[arg(long)]
    headless: bool,
}
```

### Step 2: Implement Headless Rendering

**File**: `hiwave-windows/crates/rustkit-compositor/src/lib.rs`

Add headless texture target:
```rust
pub enum RenderTarget {
    Surface(wgpu::Surface),
    Texture(wgpu::Texture), // For headless
}

impl Compositor {
    pub fn new_headless(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            // ...
        });
        // ...
    }

    pub fn read_pixels(&self) -> Vec<u8> {
        // Copy texture to CPU-readable buffer
        // Return RGBA pixels
    }
}
```

### Step 3: Update parity-capture

**File**: `hiwave-windows/crates/parity-capture/src/main.rs`

```rust
let compositor = if args.headless {
    Compositor::new_headless(&device, width, height)
} else {
    Compositor::new(&window, &device)
};

// After render:
let pixels = if args.headless {
    compositor.read_pixels()
} else {
    compositor.screenshot()
};
```

### Step 4: Test Locally

```powershell
cd hiwave-windows
cargo build --release -p parity-capture

# Test headless mode
./target/release/parity-capture.exe `
  --html-file baselines/builtins/text-basic.html `
  --output test_headless.png `
  --headless

# Compare to windowed mode
./target/release/parity-capture.exe `
  --html-file baselines/builtins/text-basic.html `
  --output test_windowed.png

# Should be identical (or very close)
```

### Step 5: Update GitHub Actions Workflow

**File**: `.github/workflows/parity-unified.yml`

Uncomment Windows parity job (lines 78-120) and add `--headless` flag:

```yaml
- name: Run parity shard
  working-directory: hiwave-windows
  shell: pwsh
  run: |
    python scripts/parity_swarm.py `
      --jobs 2 `
      --shard-index ${{ matrix.shard }} `
      --shard-count 4 `
      --scope ${{ github.event.inputs.scope || 'all' }} `
      --iterations ${{ github.event.inputs.iterations || '3' }} `
      --run-id windows-${{ github.run_number }}-shard-${{ matrix.shard }} `
      --headless  # NEW FLAG
```

### Step 6: Update parity_swarm.py

**File**: `hiwave-windows/scripts/parity_swarm.py`

Add support for `--headless` flag:
```python
def run_parity_capture(test_path, output_path, headless=False):
    cmd = [
        'target/release/parity-capture',
        '--html-file', test_path,
        '--output', output_path,
    ]
    if headless:
        cmd.append('--headless')
    subprocess.run(cmd, check=True)
```

### Step 7: Enable Aggregate Job

Update aggregate job to include Windows results:

```yaml
aggregate-all:
  needs: [macos-parity, windows-parity]  # Add windows-parity
  if: always()
  # ...
```

## Testing Plan

### Local Testing (Before CI)
1. ✅ Build parity-capture with `--headless` flag
2. ✅ Test single HTML file in headless mode
3. ✅ Compare headless vs windowed output (should match)
4. ✅ Run parity_swarm.py locally with `--headless`
5. ✅ Verify baseline comparison works

### CI Testing
1. Push changes to feature branch
2. Trigger workflow_dispatch for Windows only
3. Check if all 4 shards complete successfully
4. Review uploaded artifacts
5. Check aggregated metrics

### Validation Criteria
- ✅ All Windows shards complete without errors
- ✅ Images are generated correctly (not blank)
- ✅ Baseline comparisons work
- ✅ Visual parity percentage is reasonable (>90%)
- ✅ Badge updates correctly

## Alternative: Quick Win Without Headless

If headless is too complex initially, we can:
1. Run parity tests **locally** on Windows developer machines
2. Upload results manually or via separate script
3. Aggregate with macOS results

This gets metrics flowing while headless rendering is being implemented.

## Files to Modify

### Core Implementation:
- `hiwave-windows/crates/parity-capture/src/main.rs` - Add `--headless` flag
- `hiwave-windows/crates/rustkit-compositor/src/lib.rs` - Headless rendering
- `hiwave-windows/crates/rustkit-viewhost/src/lib.rs` - Offscreen window handling

### Scripts:
- `hiwave-windows/scripts/parity_swarm.py` - Pass `--headless` to capture
- `hiwave-windows/scripts/parity_lib.py` - Update run commands

### CI:
- `.github/workflows/parity-unified.yml` - Uncomment Windows job
- `.github/workflows/parity-unified.yml` - Add Windows to aggregate job

## Success Criteria

✅ Windows parity tests run successfully in GitHub Actions
✅ Results upload as artifacts (4 shards)
✅ Aggregate job includes Windows metrics
✅ `badges/parity-windows.svg` updates automatically
✅ `metrics/parity_results.json` includes Windows data
✅ No manual intervention required

## Dependencies

- wgpu headless rendering support (already exists in wgpu)
- Windows GitHub Actions runner (already available)
- Python 3.11+ (already in workflow)
- Playwright Chromium (already installed for oracle)

## Timeline

- **Research wgpu headless**: 1 hour
- **Implement compositor changes**: 2-3 hours
- **Update parity-capture**: 1 hour
- **Test locally**: 1 hour
- **Update CI workflow**: 30 minutes
- **Debug CI issues**: 1-2 hours

**Total**: 4-6 hours

## Notes

- macOS headless already works - can reference their implementation
- wgpu supports headless rendering via texture targets
- Screenshot capability already exists (used in hiwave-smoke)
- This unlocks Linux parity tests too (same approach)

## References

- Current workflow: `.github/workflows/parity-unified.yml`
- macOS parity: `hiwave-macos/crates/parity-capture/`
- Windows parity scripts: `hiwave-windows/scripts/parity_*.py`
- Badge: `badges/parity-windows.svg`

---

**Created**: January 17, 2026
**Assigned**: Unassigned
**Blocked By**: None (can start immediately)
**Blocks**: Linux parity tests (same technique)
