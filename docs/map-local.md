# Map Local

Map Local lets you serve static mock response files instead of forwarding requests to upstream servers. This is useful for testing client code against different response scenarios without needing actual backend services.

## Basic Concepts

A Map Local rule matches incoming requests (by host, path, method, etc.) and serves a file or directory from disk instead of forwarding the request upstream.

**File mode:** Returns the file contents verbatim.
**Directory mode:** Appends the request path to the directory and serves the matching file.

## Easiest path: upload or paste a fixture (no paths, no restart)

In the Map Local rule dialog, the **Local path** field has two helpers:

- **upload** — pick a file from your machine. It's stored server-side under `storage/map-local/` and the rule references it by name.
- **paste** — type a file name and paste content inline (great for quick JSON mocks). Same storage.

## Setup: Base Path Configuration

For Docker deployments, use `OPROXY_MAP_LOCAL_BASE_PATH` to point to a single fixtures directory. This lets you:
- Keep all mock files in one place
- Use relative paths in rules (portable across environments)
- Fail fast at rule creation time if files don't exist

### Example: Docker Compose

```yaml
services:
  oproxy:
    environment:
      OPROXY_MAP_LOCAL_BASE_PATH: /mock-responses
    volumes:
      # Mount your mock response files into the container
      - ./mock-responses:/mock-responses
```

Then create rules with relative paths like:
- `api/users.json`
- `errors/404.html`
- `responses/payment-success.json`

The middleware resolves them to `/mock-responses/api/users.json`, etc.

### Example: Docker CLI

```bash
docker run -e OPROXY_MAP_LOCAL_BASE_PATH=/mock-responses \
  -v /home/user/my-mocks:/mock-responses \
  oproxy:latest
```

### Example: Direct Configuration

In `configs/default.yaml`:
```yaml
map_local_base_path: /mock-responses
```

Then mount: `docker run -v ./mock-responses:/mock-responses oproxy:latest`

## Using the UI

1. Navigate to **Rules** → **Map Local**
2. Click **New Map Local Rule**
3. Configure location matching:
   - **Host**: domain to match (e.g., `api.example.com`)
   - **Path**: URL path glob (e.g., `/api/v1/users/*`)
   - **Methods**: HTTP methods (leave unchecked for all)
4. **Local path**: path to the file or directory
   - **With base path set**: use relative paths like `users.json` or `errors/500.html`
   - **Without base path**: use absolute paths like `/home/user/mocks/users.json`
5. Click **Save**

## File vs Directory Mode

### File Mode
If "Local path" points to a file, that file is served verbatim for all matching requests.

Example:
- Rule: matches `api.example.com/ping`
- Local path: `health/ok.json` (resolves to `/mock-responses/health/ok.json`)
- Result: `GET /ping` → returns contents of `health/ok.json`

### Directory Mode
If "Local path" points to a directory, the request path is appended to find the file.

Example:
- Rule: matches `api.example.com/api/*`
- Local path: `api` (resolves to `/mock-responses/api`)
- Request: `GET /api/users` → serves `/mock-responses/api/users` (no extension — fails)
- Request: `GET /api/users.json` → serves `/mock-responses/api/users.json` (succeeds)

**Path traversal is blocked**: If a request tries `/../../../etc/passwd`, the middleware ensures the resolved file stays within the base directory.

## Error Handling

### At Rule Creation Time
If the file/directory doesn't exist and a base path is configured:
- **Status**: 422 Unprocessable Entity
- **Message**: Shows the resolved path and suggests checking the mount

Example error:
```json
{
  "error": "file_path 'api/users.json' does not exist or is not accessible from this process. relative to base path '/mock-responses'"
}
```

### At Request Time
If a file becomes unavailable (e.g., volume unmounted):
- **Status**: 502 Bad Gateway
- **Body**: Error message indicating the path is inaccessible

This helps you catch missing files quickly during development.

## Content-Type Detection

Responses include `Content-Type` headers based on file extension:
- `.json` → `application/json`
- `.html`, `.htm` → `text/html; charset=utf-8`
- `.js`, `.mjs` → `application/javascript`
- `.css` → `text/css`
- `.png` → `image/png`
- `.jpg`, `.jpeg` → `image/jpeg`
- And 15+ more types

Binary files (images, PDFs, etc.) are served as-is.

## Relative vs Absolute Paths

A **relative** `file_path` is resolved by trying, in order:
1. `OPROXY_MAP_LOCAL_BASE_PATH` (a mounted host directory), if set
2. the managed `storage/map-local/` directory (where UI uploads land)

The first candidate that exists wins. An **absolute** `file_path` (starting with `/`) is always used verbatim.

**With `OPROXY_MAP_LOCAL_BASE_PATH` set to `/mock-responses`:**
- `users.json` → `/mock-responses/users.json` if present there, else `storage/map-local/users.json`
- `/etc/passwd` → `/etc/passwd` (absolute, ignores base path)

**Without `OPROXY_MAP_LOCAL_BASE_PATH` set:**
- `users.json` → `storage/map-local/users.json` (uploaded fixtures)
- `/home/user/mocks/users.json` → used verbatim

Absolute paths always work, so existing deployments don't break.

## Examples

### Mock API Responses

Directory structure:
```
mock-responses/
├── api/
│   ├── users.json        # GET /api/users
│   ├── users/1.json      # GET /api/users/1
│   └── posts.json        # GET /api/posts
└── errors/
    ├── 404.html          # Rewrite rule: 404 status
    └── 500.html          # Rewrite rule: 500 status
```

Rules:
- **Host**: `api.myapp.local` | **Path**: `/api/*` | **Local path**: `api`
- **Host**: `api.myapp.local` | **Path**: `/error/*` | **Local path**: `errors`

### Single File Override

Rule:
- **Host**: `payment-api.example.com` | **Path**: `/webhook/confirm`
- **Local path**: `payments/webhook-success.json`

Request: `POST /webhook/confirm` → returns the success JSON regardless of body/headers.

### Testing Errors

Rule:
- **Host**: `api.example.com` | **Path**: `/timeout*` | **Local path**: `errors/timeout.json`

Combine with a **Rewrite Rule** to set status `504` or `408`.

## Troubleshooting

**Files don't exist error at rule save:**
- Check that the path is correct relative to the base path (if set)
- Verify the file/directory actually exists on disk
- Check volume mounts in Docker if containerized

**502 Bad Gateway at runtime:**
- Volume may have become unmounted; restart the container
- Base path may have changed; re-save the rule to verify

**Wrong MIME type:**
- If the file has an unusual extension, Map Local defaults to `application/octet-stream`
- Use a **Rewrite Rule** to override `Content-Type` if needed

## Security Notes

- Map Local operates on the local filesystem only; it cannot fetch remote files
- Path traversal attacks are blocked (e.g., `/../../../etc/passwd` is rejected)
- When combined with **Breakpoints** or **Rewrite Rules**, the execution order is:
  1. Location matching
  2. Middleware chain processes the request
  3. If Map Local matches, request is short-circuited and the file is returned
  4. No upstream request is made

