use aws_lambda_events::event::apigw::{ApiGatewayProxyRequest, ApiGatewayProxyResponse};
use aws_lambda_events::encodings::{Body};
use lambda_runtime::{handler_fn, Context, Error};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Serialize, Deserialize)]
struct CustomOutput {
    message: String,
}

async fn handler(event: ApiGatewayProxyRequest, _: Context) -> Result<ApiGatewayProxyResponse, Error> {
    let response = CustomOutput {
        message: "Hello from Rust!".to_string(),
    };

    let response_json = serde_json::to_string(&response)?;

    Ok(ApiGatewayProxyResponse {
        status_code: 200,
        headers: Default::default(),
        body: Some(Body::from(response_json)),
        is_base64_encoded: Some(false),
        multi_value_headers: Default::default(),
    })
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let func = handler_fn(handler);
    lambda_runtime::run(func).await?;
    Ok(())
}
