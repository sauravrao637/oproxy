# 🚀 oproxy

**A HTTP/HTTPS proxy for inspecting, debugging, and manipulating network traffic.** Includes a zero-dependency web dashboard.

![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)
![Language: Rust](https://img.shields.io/badge/language-Rust-orange.svg)
![Docker: GHCR](https://img.shields.io/badge/Docker-GHCR-green.svg)

---

## ✨ Key Features

* **🔍 Traffic Inspection** — Capture and search every request/response in a live session log.
* **🔀 Smart Routing** — Redirect traffic at the proxy level without client-side changes.
* **✍️ Request/Response Rewrites** — Manipulate request/response on the fly.
* **⏸️ Breakpoints** — Pause, inspect, and modify requests mid-flight.
* **🐢 Network Throttling** — Simulate slow connections with latency injection.
* **💻 Management UI** — Built-in dashboard.

---

## 🚀 Quick Start

### 1. Run with Docker (Recommended)
The fastest way to get started using the official image from GitHub Container Registry:

```bash
docker run -d \
  --name oproxy \
  -p 8080:8080 \
  -v $(pwd)/configs:/configs \
  ghcr.io/sauravrao637/oproxy:latest
```

### 2. Run from Source
**Prerequisites:** Rust toolchain (`rustup`, `cargo`)

```bash
git clone [https://github.com/sauravrao637/oproxy.git](https://github.com/sauravrao637/oproxy.git)
cd oproxy
cargo run --release
```

**Access the Dashboard:** Open [http://localhost:8080](http://localhost:8080) in your browser.

---

## 🛠️ Configuration

`oproxy` uses a layered configuration system: **Environment Variables > YAML Config > Defaults**.

### Client Setup
Point your HTTP/HTTPS proxy settings to `localhost:8080`.

| Tool | Command / Setting |
| :--- | :--- |
| **curl** | `curl -x http://localhost:8080 http://example.com` |
| **Browser** | Settings → Network → Manual Proxy → `localhost:8080` |
| **HTTPS (MITM)** | Download CA: `curl http://localhost:8080/admin/ca -o oproxy-ca.crt` |

> **Note:** For HTTPS interception, ensure `mitm.enabled: true` is set in your config, and trust the downloaded `oproxy-ca.crt` in your system/browser.

---

<details>
<summary><h2>📡 Management API Reference</h2></summary>

The internal API is only accessible from `localhost`.

| Category | Endpoint | Method | Description |
| :--- | :--- | :--- | :--- |
| **Sessions** | `/api/sessions` | `GET` | List captured traffic |
| **Sessions** | `/api/sessions/:id` | `GET` | Full detail for one session |
| **Routes** | `/admin/routes` | `POST` | Update routing table |
| **Rewrites** | `/admin/rewrites` | `POST` | Add regex rewrite rules |
| **Breakpoints**| `/admin/breakpoints` | `GET` | List active breakpoints |
| **Throttling** | `/admin/throttling` | `POST`| Update throttling config |
| **System** | `/admin/ca` | `GET` | Export Root CA (.pem) |

</details>

<details>
<summary><h2>📂 Project Structure (For Contributors)</h2></summary>

The project follows a modular Rust architecture designed for extensibility:

```text
oproxy/
├── src/
│   ├── core/                # ProxyEngine, MITM logic, and connection forwarding
│   ├── middleware/          # The "Plugin" system (Routing, Rewrites, Breakpoints)
│   ├── management.rs        # Axum web server & Admin API
│   ├── index.html           # Inlined PWA Management UI
│   └── certs/               # CA and dynamic cert generation logic
├── configs/                 # Default YAML templates
└── Dockerfile               # Multi-arch optimized build
```

</details>

---

## 🤝 Contributing

We welcome contributions! Please follow these steps:
1.  **Fork** the repository.
2.  **Create** a feature branch (`git checkout -b feature/amazing-feature`).
3.  **Test** your changes (`cargo test`).
4.  **Lint** your code (`cargo clippy -- -D warnings`).
5.  **Submit** a Pull Request.

---

## 📄 License
Distributed under the [**MIT License**](LICENSE).