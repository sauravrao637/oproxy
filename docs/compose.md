# Compose

Compose is the UI surface for creating and replaying manual HTTP requests through `/admin/forward`.

## Creating Requests

Open Compose and create a new tab. A request can include:

- method: `GET`, `POST`, `PUT`, `PATCH`, `DELETE`, `HEAD`, `OPTIONS`
- absolute `http://` or `https://` URL
- enabled/disabled header rows
- enabled/disabled query parameter rows
- auth: none, bearer token, or basic auth
- raw body with content type `application/json`, `text/plain`, `text/html`, or `application/xml`

Compose refuses to send URLs that are empty, invalid, non-HTTP(S), or still contain unresolved `{{variables}}`.

Equivalent API call:

```bash
curl -X POST http://127.0.0.1:8080/admin/forward \
  -H 'content-type: application/json' \
  -d '{
    "method": "POST",
    "url": "https://example.com/api",
    "headers": {
      "content-type": "application/json"
    },
    "body": "{\"ok\":true}"
  }'
```

## Collections

Compose collections are saved in browser local storage under `oproxy.compose.workspace.v1`. They are not written to the server `storage_path`.

Using UI one can:

- create collections
- rename collections
- save requests into collections
- open saved requests in tabs
- export the Compose workspace JSON as `oproxy-collections.json`


## Variables

Variables are also browser-local. Enabled variables replace `{{name}}` in URLs, headers, params, auth values, and bodies before sending.

Example URL:

```text
https://{{base}}/api/users
```

If `base` is unresolved or disabled, the request is not sent.

## cURL Import and Export

Compose has a cURL button that copies the current request as a cURL command.
Paste a `curl ...` command into the URL field to populate the active request tab.

The parser accepts HTTP and HTTPS URLs. It rejects input that does not start with `curl`.

## Limitations

- Compose sends through the admin API, not through a browser-configured proxy connection.
- Compose collections and variables are per browser profile and admin origin.
- Compose response timing is UI-measured elapsed time, not a full network phase waterfall.
- Admin egress restrictions can block `/admin/forward` when remote admin is enabled.
