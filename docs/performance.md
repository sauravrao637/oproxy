# Performance and Soak Testing

`scripts/soak-test.sh` measures baseline latency and sustained-load memory growth. It exercises small and large HTTP bodies with configurable concurrency and duration.

## What it does

1. Starts a small local HTTP origin and an isolated oproxy instance (its own
   port, storage directory, CA directory - it never touches your real
   `storage/` or `certs/`).
2. **Baseline**: fires a configurable number of requests through the proxy at
   a configurable concurrency (defaults: 500 requests / 50 concurrent) and
   reports p50/p95/p99 latency and failure count. Uses `hey` or `wrk` if
   either is installed; otherwise falls back to a built-in concurrent-curl
   runner with coarser percentiles.
3. **Soak**: sustained mixed traffic (small requests, with roughly 1 in 10
   hitting a large response body to exercise the streaming path - see
   `stream_threshold_bytes` in [configuration.md](configuration.md)) for a
   configurable duration (default 5 minutes), sampling the oproxy process's
   RSS every few seconds. Fails if RSS grows past a configurable ratio of its
   first post-warmup sample, or if any soak request fails.

Exit code is nonzero if the baseline had failures, the soak phase had
failures, or RSS grew past the configured ratio.

## Running it

```bash
cargo build --release
scripts/soak-test.sh
```

Options (all also settable via matching env vars, e.g. `REQUESTS=1000`):

```bash
scripts/soak-test.sh \
  --bin target/release/oproxy \
  --requests 500 \
  --concurrency 50 \
  --soak-seconds 300 \
  --large-body-bytes 5242880
```

`--bin` defaults to `target/release/oproxy`; point it at a debug build or a
packaged binary if you're testing something other than a fresh release
build.

## Reading the numbers

The specific latency/throughput numbers this script prints are a measurement
of *this machine, this run* - they are not a portable SLA and shouldn't be
copied into a doc as "the" baseline. Run it once on your own reference
hardware or CI runner, record that as your baseline, and compare future runs
against it (or wire it into CI to fail a build on regression) rather than
against numbers quoted here.

## Why shell + curl (and not k6 or a Rust bench harness)

- No extra runtime (Node/Go) or crate to vendor - curl and bash are already
  assumed available everywhere this project documents (Docker, CI, local
  dev).
- A Rust bench harness (criterion or a `benches/` crate) would need to
  compile with the rest of the workspace, which means perf tooling could
  never run unless the whole workspace builds - that defeats the point of a
  quick regression check against a released binary.
- `hey`/`wrk` give nicer latency histograms when installed; the built-in
  fallback keeps the script self-contained when they aren't.

## Proxy bypass protection

Both the proxy under test and the origin server run on `127.0.0.1`. Many
shells and CI environments set `no_proxy`/`NO_PROXY` to include
`127.0.0.1`/`localhost` (this project's own Docker/CI environment does, and
it's a common default well beyond this project). `curl` honors that env var
for the *destination* host even when `-x`/`--proxy` is passed explicitly -
so each proxied curl call passes `--noproxy ''` to force traffic through `-x` regardless of the ambient `no_proxy` setting. Keep this flag when adapting the script; otherwise the test may bypass the proxy without failing.

## Not covered yet

The script does not cover slow clients, cancellation, or concurrent WebSocket load. Those scenarios require protocol-specific tooling for stalled bodies, mid-stream disconnects, and concurrent `ws://` sessions.
