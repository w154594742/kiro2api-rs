# kiro-rs

English | [中文](./README.md)

An Anthropic Claude API compatible proxy service written in Rust, converting Anthropic API requests to Kiro API requests.

> ⭐ If this project helps you, please give it a Star!

## Table of Contents

- [Features](#features)
- [Supported API Endpoints](#supported-api-endpoints)
- [Quick Start](#quick-start)
- [Running Modes](#running-modes)
- [Environment Variables](#environment-variables)
- [Docker Deployment](#docker-deployment)
- [Zeabur Deployment](#zeabur-deployment)
- [Web Management Panel](#web-management-panel)
- [Configuration](#configuration)
- [Usage Examples](#usage-examples)
- [Advanced Features](#advanced-features)
- [Tech Stack](#tech-stack)
- [License](#license)
- [Acknowledgments](#acknowledgments)

## Features

- **Anthropic API Compatible**: Full support for Anthropic Claude API format
- **Streaming Response**: SSE (Server-Sent Events) streaming output support
- **Auto Token Refresh**: Automatic OAuth Token management and refresh
- **Thinking Mode**: Support for Claude's extended thinking feature
- **Tool Calling**: Full support for function calling / tool use
- **Multi-Model Support**: Support for Sonnet, Opus, Haiku series models
- **Account Pool Mode**: Multi-account rotation and load balancing
- **Web Management Panel**: Visual account management and status monitoring
- **Quota Management**: Real-time account quota viewing
- **Request Logging**: Request history and statistics
- **Auto Error Handling**: Auto cooldown on rate limiting, auto exhausted state on monthly quota limits, auto disable on suspension

## Supported API Endpoints

### Anthropic API

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/v1/models` | GET | Get available models list |
| `/v1/messages` | POST | Create message (conversation) |
| `/v1/messages/count_tokens` | POST | Estimate token count |

### Management API (Authentication Required)

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/status` | GET | Get service status |
| `/api/accounts` | GET/POST | Get/Add accounts |
| `/api/accounts/import` | POST | Import Kiro JSON credentials |
| `/api/accounts/{id}` | DELETE | Delete account |
| `/api/accounts/{id}/enable` | POST | Enable account |
| `/api/accounts/{id}/disable` | POST | Disable account |
| `/api/accounts/{id}/usage` | GET | Get account quota |
| `/api/accounts/{id}/usage/refresh` | POST | Refresh account quota |
| `/api/strategy` | GET/POST | Get/Set load balancing strategy |
| `/api/logs` | GET | Get request logs |
| `/api/logs/stats` | GET | Get request statistics |
| `/api/usage/refresh` | POST | Refresh all account quotas |

## Quick Start

### 1. Build the Project

```bash
cargo build --release
```

### 2. Configuration File

Create `config.json` configuration file:

```json
{
   "host": "0.0.0.0",
   "port": 8080,
   "apiKey": "sk-your-custom-api-key",
   "region": "us-east-1"
}
```

### 3. Credentials File

Create `credentials.json` credentials file:

**Social Authentication (Minimal Config):**
```json
{
   "refreshToken": "XXXXXXXXXXXXXXXX",
   "expiresAt": "2025-12-31T02:32:45.144Z",
   "authMethod": "social"
}
```

**IdC / BuilderId Authentication:**
```json
{
   "refreshToken": "XXXXXXXXXXXXXXXX",
   "expiresAt": "2025-12-31T02:32:45.144Z",
   "authMethod": "idc",
   "clientId": "xxxxxxxxx",
   "clientSecret": "xxxxxxxxx"
}
```

### 4. Start the Service

**Single Account Mode:**
```bash
./target/release/kiro-rs
```

**Account Pool Mode (with Web Management Panel):**

*Linux / macOS:*
```bash
POOL_MODE=true ./target/release/kiro-rs
```

*Windows PowerShell:*
```powershell
$env:POOL_MODE="true"; ./target/release/kiro-rs
```

*Windows CMD:*
```cmd
set POOL_MODE=true
target\release\kiro-rs
```

## Running Modes

### Single Account Mode (Default)

Runs with a single credentials file, suitable for personal use.

### Account Pool Mode

Enable by setting `POOL_MODE=true`, supports:
- Multi-account management
- Round-robin / Random / Least-used load balancing strategies
- Account status tracking (Active/Cooldown/Exhausted/Disabled)
- Web management panel (visit `http://service-address/`)
- Persistent account storage

> **Note**: When account pool mode is enabled, the system **no longer** reads the `credentials.json` file.
> Account data is stored in `data/accounts.json`. You can:
> 1. Add accounts via Web management panel after starting the service (Recommended)
> 2. Manually create `data/accounts.json` file (refer to `accounts.example.json` in root directory)

## Environment Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `HOST` | Listen address | `0.0.0.0` |
| `PORT` | Listen port | `8080` |
| `API_KEY` | API key | - |
| `REGION` | AWS region | `us-east-1` |
| `POOL_MODE` | Enable account pool mode | `false` |
| `DATA_DIR` | Data storage directory | `./data` |
| `REFRESH_TOKEN` | OAuth refresh token | - |
| `AUTH_METHOD` | Auth method (social/idc) | - |
| `CLIENT_ID` | IdC client ID | - |
| `CLIENT_SECRET` | IdC client secret | - |

## Docker Deployment

```bash
docker build -t kiro-rs .
docker run -d \
  -p 8080:8080 \
  -e API_KEY=sk-your-key \
  -e POOL_MODE=true \
  -v /path/to/data:/app/data \
  kiro-rs
```

## Zeabur Deployment

1. Fork this repository or import directly
2. Add persistent storage volume, mount to `/app/data`
3. Set environment variables:
   ```
   POOL_MODE=true
   API_KEY=sk-your-api-key
   DATA_DIR=/app/data
   ```
4. After deployment, visit the service address to see the management panel

## Web Management Panel

In account pool mode, visit the service root path to open the management panel (API key login required):

### Feature Overview

- 📊 **Real-time Status Monitoring** - Uptime, account status, request statistics, token usage
- 👥 **Account Management** - Add, import, enable/disable, delete accounts
- 📈 **Quota Viewing** - Real-time refresh of account remaining quota and usage progress
- 📝 **Request Logs** - View last 100 request history (persists last 1000 entries)
- 🔄 **Load Balancing** - Switch between round-robin/random/least-used/sequential-exhaust strategies
- 🔐 **Security Authentication** - API key protected management panel

### Quota Management

Click the 🔄 button in the account list to refresh individual account quota, or click "Refresh Quota" in the toolbar to batch refresh all accounts.

Quota progress bar colors:
- 🟢 Green: Remaining > 30%
- 🟡 Yellow: Remaining 10-30%
- 🔴 Red: Remaining < 10%

### Auto Error Handling

- **429 Rate Limit Error**: Account automatically enters 5-minute cooldown
- **402 Monthly Quota Exhausted**: Account automatically marked as exhausted (hourly recovery scan)
- **403 Suspension Error**: Account automatically disabled
- Error counts update in real-time for troubleshooting problematic accounts

### Tiered Recovery Scans

- **Cooldown accounts**: scanned every 15 minutes and auto-recovered when ready
- **Exhausted accounts**: scanned every 1 hour and auto-recovered after quota returns

### Data Persistence

In account pool mode, the following data is automatically saved to `DATA_DIR`:
- `accounts.json` - Account information and status
- `request_logs.json` - Request logs (max 1000 entries)

### Import Kiro Credentials

Supports directly pasting complete JSON exported from Kiro IDE:

```json
{
  "email": "xxx@example.com",
  "provider": "BuilderId",
  "refreshToken": "aorAAAAA...",
  "clientId": "...",
  "clientSecret": "...",
  "region": "us-east-1"
}
```

The system will automatically identify the authentication method and extract the account name.

## Configuration

### config.json

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `host` | string | `0.0.0.0` | Service listen address |
| `port` | number | `8080` | Service listen port |
| `apiKey` | string | - | Custom API Key |
| `region` | string | `us-east-1` | AWS region |
| `kiroVersion` | string | `0.8.0` | Kiro version |
| `machineId` | string | Auto-generated | Custom machine ID |
| `proxyUrl` | string | - | HTTP/SOCKS5 proxy |

### credentials.json

| Field | Type | Description |
|-------|------|-------------|
| `accessToken` | string | OAuth access token (optional) |
| `refreshToken` | string | OAuth refresh token |
| `profileArn` | string | AWS Profile ARN (optional) |
| `expiresAt` | string | Token expiration time |
| `authMethod` | string | Auth method (social/idc) |
| `clientId` | string | IdC client ID |
| `clientSecret` | string | IdC client secret |

## Usage Examples

```bash
curl http://127.0.0.1:8080/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: sk-your-api-key" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "max_tokens": 1024,
    "messages": [
      {"role": "user", "content": "Hello!"}
    ]
  }'
```

## Advanced Features

### Thinking Mode

```json
{
  "model": "claude-sonnet-4-20250514",
  "max_tokens": 16000,
  "thinking": {
    "type": "enabled",
    "budget_tokens": 10000
  },
  "messages": [...]
}
```

### Tool Calling

```json
{
  "model": "claude-sonnet-4-20250514",
  "max_tokens": 1024,
  "tools": [
    {
      "name": "get_weather",
      "description": "Get weather",
      "input_schema": {
        "type": "object",
        "properties": {
          "city": {"type": "string"}
        }
      }
    }
  ],
  "messages": [...]
}
```

### Streaming Response

```json
{
  "model": "claude-sonnet-4-20250514",
  "max_tokens": 1024,
  "stream": true,
  "messages": [...]
}
```

## Tech Stack

- **Web Framework**: Axum 0.8
- **Async Runtime**: Tokio
- **HTTP Client**: Reqwest (rustls)
- **Serialization**: Serde
- **Logging**: tracing

## License

MIT
