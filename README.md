# choko

Rust Serverless Microframework for AWS Lambda, inspired by [AWS Chalice](https://github.com/aws/chalice).

A traditional Japanese vessel, like a cup, is called "choko" and written "猪口".

🐗

## Features

- Declarative routing with path parameters (`/users/{user_id}`)
- Automatic JSON request body parsing
- Fluent response builder (`Response::json(...).with_status(201)`)
- Built-in 404 / 405 / 500 error responses
- Runs on API Gateway (REST API) + Lambda proxy integration

## Quick Start

Add `choko` to your project:

```toml
[dependencies]
choko = { git = "https://github.com/velocitylabo/choko" }
tokio = { version = "1", features = ["full"] }
serde_json = "1.0"
```

Create `src/main.rs`:

```rust
use choko::{Choko, Response, Error, serde_json};
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Error> {
    let mut app = Choko::new("my-api");

    app.route("/", &["GET"], |_req| async {
        Ok(Response::json(json!({"message": "Hello, choko!"})))
    });

    app.run().await
}
```

## Usage

### Defining Routes

```rust
// GET /users
app.route("/users", &["GET"], |_req| async {
    Ok(Response::json(json!({"users": []})))
});

// POST /users
app.route("/users", &["POST"], |req| async move {
    let name = req.json_body
        .as_ref()
        .and_then(|v| v.get("name"))
        .and_then(|v| v.as_str())
        .unwrap_or("anonymous");
    Ok(Response::json(json!({"created": name})).with_status(201))
});

// Multiple methods on one route
app.route("/health", &["GET", "HEAD"], |_req| async {
    Ok(Response::json(json!({"status": "ok"})))
});
```

### Path Parameters

Use `{param}` syntax to capture URL segments:

```rust
app.route("/users/{user_id}", &["GET"], |req| async move {
    let user_id = req.path_params.get("user_id").unwrap();
    Ok(Response::json(json!({"user_id": user_id})))
});

app.route("/users/{user_id}/posts/{post_id}", &["GET"], |req| async move {
    let user_id = req.path_params.get("user_id").unwrap();
    let post_id = req.path_params.get("post_id").unwrap();
    Ok(Response::json(json!({
        "user_id": user_id,
        "post_id": post_id
    })))
});
```

### Request Object

The handler receives a `Request` with:

| Field | Type | Description |
|---|---|---|
| `path_params` | `HashMap<String, String>` | URL path parameters |
| `query_params` | `HashMap<String, Vec<String>>` | Query string parameters (multi-value) |
| `headers` | `HashMap<String, String>` | HTTP headers |
| `body` | `Option<String>` | Raw request body |
| `json_body` | `Option<Value>` | Parsed JSON body |

```rust
app.route("/search", &["GET"], |req| async move {
    let query = req.query_params.get("q")
        .and_then(|v| v.first())
        .cloned()
        .unwrap_or_default();
    Ok(Response::json(json!({"query": query})))
});
```

### Response Builder

```rust
// 200 JSON (default)
Response::json(json!({"ok": true}))

// Custom status code
Response::json(json!({"id": 1})).with_status(201)

// Custom headers
Response::json(json!({}))
    .with_header("X-Request-Id", "abc-123")
    .with_header("Cache-Control", "no-cache")
```

### Error Handling

If a handler returns `Err`, choko automatically responds with HTTP 500:

```rust
app.route("/risky", &["GET"], |_req| async {
    let result = tokio::fs::read_to_string("/tmp/data.json").await;
    match result {
        Ok(data) => Ok(Response::json(json!({"data": data}))),
        Err(e) => Err(e.into()),  // -> 500 {"error": "Internal Server Error"}
    }
});
```

Unmatched paths return 404, and wrong HTTP methods return 405.

## Build & Deploy

### Prerequisites

- [cargo-make](https://github.com/sagiegurari/cargo-make): `cargo install cargo-make`
- AWS CLI (configured with valid credentials)

### Manual packaging

```bash
# Build release binary
cargo make build-release

# Run tests
cargo make test

# Create Lambda deployment zip (bootstrap.zip)
cargo make package
```

### One-command deploy with `choko` CLI

Install the CLI:

```bash
cargo install choko --features cli
```

Deploy to AWS Lambda + API Gateway:

```bash
choko deploy --role-arn arn:aws:iam::123456789012:role/my-lambda-role
```

This single command will:

1. Build a release binary and package `bootstrap.zip`
2. Create (or update) a Lambda function using the `provided.al2023` runtime
3. Create (or update) an API Gateway REST API with `{proxy+}` integration
4. Deploy to a stage and print the endpoint URL

#### CLI options

| Option | Env var | Default | Description |
|---|---|---|---|
| `--role-arn` | `CHOKO_ROLE_ARN` | *(required)* | IAM role ARN for the Lambda function |
| `--region` | `AWS_DEFAULT_REGION` | `ap-northeast-1` | AWS region |
| `--function-name` | | Cargo.toml `name` | Lambda function name |
| `--stage` | | `prod` | API Gateway stage name |
| `--memory` | | `128` | Lambda memory (MB) |
| `--timeout` | | `30` | Lambda timeout (seconds) |
| `--target` | | `x86_64-unknown-linux-gnu` | Rust target triple for cross-compilation |

> `--target` に `aarch64-unknown-linux-gnu` を指定すると、Lambda の architecture が自動的に `arm64` に設定されます。

#### Package only (no deploy)

```bash
choko package
choko package --target aarch64-unknown-linux-gnu  # ARM64 向け
```

## License

MIT
