use clap::{Parser, Subcommand};
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

#[derive(Parser)]
#[command(
    name = "choko",
    version,
    about = "Choko CLI - Deploy Rust Lambda applications to AWS"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build and create bootstrap.zip for Lambda deployment
    Package(PackageArgs),
    /// Deploy the application to AWS Lambda + API Gateway
    Deploy(DeployArgs),
}

#[derive(clap::Args)]
struct PackageArgs {
    /// Rust target triple for cross-compilation (e.g. aarch64-unknown-linux-gnu)
    #[arg(long, default_value = "x86_64-unknown-linux-gnu")]
    target: String,
}

#[derive(clap::Args)]
struct DeployArgs {
    /// AWS region
    #[arg(long, env = "AWS_DEFAULT_REGION", default_value = "ap-northeast-1")]
    region: String,

    /// IAM role ARN for the Lambda function
    #[arg(long, env = "CHOKO_ROLE_ARN")]
    role_arn: String,

    /// Lambda function name (defaults to Cargo.toml package name)
    #[arg(long)]
    function_name: Option<String>,

    /// API Gateway stage name
    #[arg(long, default_value = "prod")]
    stage: String,

    /// Lambda memory size in MB
    #[arg(long, default_value = "128")]
    memory: u32,

    /// Lambda timeout in seconds
    #[arg(long, default_value = "30")]
    timeout: u32,

    /// Rust target triple for cross-compilation (e.g. aarch64-unknown-linux-gnu)
    #[arg(long, default_value = "x86_64-unknown-linux-gnu")]
    target: String,
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Package(args) => package(&args.target),
        Commands::Deploy(args) => deploy(args),
    };
    if let Err(e) = result {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn get_package_name() -> Result<String, String> {
    let content =
        fs::read_to_string("Cargo.toml").map_err(|e| format!("Failed to read Cargo.toml: {e}"))?;
    let parsed: toml::Value = content
        .parse()
        .map_err(|e| format!("Failed to parse Cargo.toml: {e}"))?;
    parsed
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "package.name not found in Cargo.toml".to_string())
}

/// Run an external command and return stdout on success.
fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(cmd)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Failed to execute `{cmd}`: {e}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let args_str = args.join(" ");
        Err(format!("`{cmd} {args_str}` failed: {stderr}"))
    }
}

/// Run a command, inheriting stdout/stderr so the user sees progress.
fn run_visible(cmd: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(cmd)
        .args(args)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| format!("Failed to execute `{cmd}`: {e}"))?;

    if status.success() {
        Ok(())
    } else {
        let args_str = args.join(" ");
        Err(format!("`{cmd} {args_str}` exited with {status}"))
    }
}

/// Run an AWS CLI command with `--output json` appended automatically.
fn aws(args: &[&str]) -> Result<String, String> {
    let mut full_args = args.to_vec();
    full_args.extend_from_slice(&["--output", "json"]);
    run("aws", &full_args)
}

/// Shorthand for parsing AWS CLI JSON output.
fn parse_json(raw: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str(raw).map_err(|e| format!("Failed to parse JSON: {e}"))
}

/// Derive the Lambda architecture string from a Rust target triple.
fn lambda_arch(target: &str) -> &str {
    if target.starts_with("aarch64") {
        "arm64"
    } else {
        "x86_64"
    }
}

// ---------------------------------------------------------------------------
// Package
// ---------------------------------------------------------------------------

fn package(target: &str) -> Result<(), String> {
    let pkg = get_package_name()?;
    // Cargo converts hyphens to underscores in binary names
    let bin_name = pkg.replace('-', "_");

    println!("Building release binary for {target}...");
    run_visible("cargo", &["build", "--release", "--target", target])?;

    let bin_path = format!("target/{target}/release/{bin_name}");
    if !Path::new(&bin_path).exists() {
        return Err(format!("Binary not found at {bin_path}"));
    }

    println!("Creating bootstrap.zip...");
    create_bootstrap_zip(&bin_path)?;

    println!("Created bootstrap.zip");
    Ok(())
}

fn create_bootstrap_zip(binary_path: &str) -> Result<(), String> {
    let binary_data =
        fs::read(binary_path).map_err(|e| format!("Failed to read {binary_path}: {e}"))?;

    let file = fs::File::create("bootstrap.zip")
        .map_err(|e| format!("Failed to create bootstrap.zip: {e}"))?;

    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);

    zip.start_file("bootstrap", options)
        .map_err(|e| format!("Failed to add file to zip: {e}"))?;

    zip.write_all(&binary_data)
        .map_err(|e| format!("Failed to write zip data: {e}"))?;

    zip.finish()
        .map_err(|e| format!("Failed to finalize zip: {e}"))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Deploy
// ---------------------------------------------------------------------------

fn deploy(args: DeployArgs) -> Result<(), String> {
    let pkg = get_package_name()?;
    let function_name = args.function_name.as_deref().unwrap_or(&pkg);
    let region = &args.region;

    // 1. Package
    package(&args.target)?;

    // 2. Lambda — returns the function ARN for API Gateway integration
    let function_arn = ensure_lambda(function_name, region, &args)?;

    // 3. API Gateway
    let api_id = ensure_api_gateway(function_name, region)?;
    setup_proxy_integration(&api_id, &function_arn, function_name, region, &args.stage)?;

    let endpoint = format!(
        "https://{api_id}.execute-api.{region}.amazonaws.com/{}",
        args.stage
    );
    println!();
    println!("Deployed successfully!");
    println!("  Function : {function_name}");
    println!("  API GW   : {api_id}");
    println!("  Endpoint : {endpoint}");

    Ok(())
}

// ---------------------------------------------------------------------------
// Lambda helpers
// ---------------------------------------------------------------------------

fn lambda_exists(name: &str, region: &str) -> bool {
    aws(&[
        "lambda",
        "get-function",
        "--function-name",
        name,
        "--region",
        region,
    ])
    .is_ok()
}

/// Create or update the Lambda function. Returns the function ARN.
fn ensure_lambda(name: &str, region: &str, args: &DeployArgs) -> Result<String, String> {
    let mem = args.memory.to_string();
    let tout = args.timeout.to_string();
    let arch = lambda_arch(&args.target);

    let arn = if lambda_exists(name, region) {
        println!("Updating Lambda function: {name}");
        let raw = aws(&[
            "lambda",
            "update-function-code",
            "--function-name",
            name,
            "--zip-file",
            "fileb://bootstrap.zip",
            "--architectures",
            arch,
            "--region",
            region,
        ])?;
        let resp = parse_json(&raw)?;
        let arn = resp
            .get("FunctionArn")
            .and_then(|v| v.as_str())
            .ok_or("update-function-code response missing FunctionArn")?
            .to_string();

        // Wait for code update to finish before changing configuration
        aws(&[
            "lambda",
            "wait",
            "function-updated-v2",
            "--function-name",
            name,
            "--region",
            region,
        ])?;

        aws(&[
            "lambda",
            "update-function-configuration",
            "--function-name",
            name,
            "--memory-size",
            &mem,
            "--timeout",
            &tout,
            "--region",
            region,
        ])?;

        arn
    } else {
        println!("Creating Lambda function: {name}");
        let raw = aws(&[
            "lambda",
            "create-function",
            "--function-name",
            name,
            "--runtime",
            "provided.al2023",
            "--handler",
            "bootstrap",
            "--architectures",
            arch,
            "--role",
            &args.role_arn,
            "--zip-file",
            "fileb://bootstrap.zip",
            "--memory-size",
            &mem,
            "--timeout",
            &tout,
            "--region",
            region,
        ])?;
        let resp = parse_json(&raw)?;
        let arn = resp
            .get("FunctionArn")
            .and_then(|v| v.as_str())
            .ok_or("create-function response missing FunctionArn")?
            .to_string();

        // Wait until the function is active
        aws(&[
            "lambda",
            "wait",
            "function-active-v2",
            "--function-name",
            name,
            "--region",
            region,
        ])?;

        arn
    };

    Ok(arn)
}

// ---------------------------------------------------------------------------
// API Gateway helpers
// ---------------------------------------------------------------------------

fn ensure_api_gateway(function_name: &str, region: &str) -> Result<String, String> {
    let api_name = format!("choko-{function_name}");
    let raw = aws(&["apigateway", "get-rest-apis", "--region", region])?;
    let apis = parse_json(&raw)?;

    if let Some(items) = apis.get("items").and_then(|v| v.as_array()) {
        for item in items {
            if item.get("name").and_then(|n| n.as_str()) == Some(&api_name) {
                let id = item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or("REST API missing id")?;
                println!("Using existing API Gateway: {api_name} ({id})");
                return Ok(id.to_string());
            }
        }
    }

    println!("Creating API Gateway: {api_name}");
    let raw = aws(&[
        "apigateway",
        "create-rest-api",
        "--name",
        &api_name,
        "--endpoint-configuration",
        "types=REGIONAL",
        "--region",
        region,
    ])?;
    let api = parse_json(&raw)?;
    api.get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "create-rest-api response missing id".to_string())
}

fn setup_proxy_integration(
    api_id: &str,
    function_arn: &str,
    function_name: &str,
    region: &str,
    stage: &str,
) -> Result<(), String> {
    // --- resolve resource IDs ---
    let raw = aws(&[
        "apigateway",
        "get-resources",
        "--rest-api-id",
        api_id,
        "--region",
        region,
    ])?;
    let resources = parse_json(&raw)?;
    let items = resources
        .get("items")
        .and_then(|v| v.as_array())
        .ok_or("get-resources returned no items")?;

    let root_id = items
        .iter()
        .find(|r| r.get("path").and_then(|p| p.as_str()) == Some("/"))
        .and_then(|r| r.get("id"))
        .and_then(|v| v.as_str())
        .ok_or("root resource not found")?
        .to_string();

    let proxy_id = match items
        .iter()
        .find(|r| r.get("pathPart").and_then(|p| p.as_str()) == Some("{proxy+}"))
        .and_then(|r| r.get("id"))
        .and_then(|v| v.as_str())
    {
        Some(id) => id.to_string(),
        None => {
            println!("Creating {{proxy+}} resource...");
            let raw = aws(&[
                "apigateway",
                "create-resource",
                "--rest-api-id",
                api_id,
                "--parent-id",
                &root_id,
                "--path-part",
                "{proxy+}",
                "--region",
                region,
            ])?;
            parse_json(&raw)?
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or("create-resource response missing id")?
                .to_string()
        }
    };

    // --- integration URI from the ARN returned by ensure_lambda ---
    let uri = format!(
        "arn:aws:apigateway:{region}:lambda:path/2015-03-31/functions/{function_arn}/invocations"
    );

    // --- wire up root (/) and {proxy+} ---
    println!("Setting up Lambda proxy integration...");
    for resource_id in [&root_id, &proxy_id] {
        // put-method may fail if the method already exists — only warn on unexpected errors
        if let Err(e) = aws(&[
            "apigateway",
            "put-method",
            "--rest-api-id",
            api_id,
            "--resource-id",
            resource_id,
            "--http-method",
            "ANY",
            "--authorization-type",
            "NONE",
            "--region",
            region,
        ]) {
            if !e.contains("ConflictException") {
                eprintln!("Warning: put-method failed: {e}");
            }
        }

        aws(&[
            "apigateway",
            "put-integration",
            "--rest-api-id",
            api_id,
            "--resource-id",
            resource_id,
            "--http-method",
            "ANY",
            "--type",
            "AWS_PROXY",
            "--integration-http-method",
            "POST",
            "--uri",
            &uri,
            "--region",
            region,
        ])?;
    }

    // --- Lambda invoke permission for API Gateway ---
    let account_id = get_account_id(region)?;
    let source_arn = format!("arn:aws:execute-api:{region}:{account_id}:{api_id}/*");

    // remove-permission may fail if the statement doesn't exist yet
    if let Err(e) = aws(&[
        "lambda",
        "remove-permission",
        "--function-name",
        function_name,
        "--statement-id",
        "choko-apigateway",
        "--region",
        region,
    ]) {
        if !e.contains("ResourceNotFoundException") {
            eprintln!("Warning: remove-permission failed: {e}");
        }
    }

    aws(&[
        "lambda",
        "add-permission",
        "--function-name",
        function_name,
        "--statement-id",
        "choko-apigateway",
        "--action",
        "lambda:InvokeFunction",
        "--principal",
        "apigateway.amazonaws.com",
        "--source-arn",
        &source_arn,
        "--region",
        region,
    ])?;

    // --- deploy stage ---
    println!("Deploying to stage: {stage}");
    aws(&[
        "apigateway",
        "create-deployment",
        "--rest-api-id",
        api_id,
        "--stage-name",
        stage,
        "--region",
        region,
    ])?;

    Ok(())
}

fn get_account_id(region: &str) -> Result<String, String> {
    let raw = aws(&["sts", "get-caller-identity", "--region", region])?;
    parse_json(&raw)?
        .get("Account")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "Could not resolve AWS account ID".to_string())
}
