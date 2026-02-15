use aws_lambda_events::encodings::Body;
use aws_lambda_events::event::apigw::{ApiGatewayProxyRequest, ApiGatewayProxyResponse};
use lambda_runtime::{service_fn, Error, LambdaEvent};
use serde_json::Value;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

/// A request object passed to route handlers.
pub struct Request {
    /// Path parameters extracted from the URL pattern (e.g., `{user_id}` -> "123").
    pub path_params: HashMap<String, String>,
    /// Query string parameters.
    pub query_params: HashMap<String, String>,
    /// HTTP headers.
    pub headers: HashMap<String, String>,
    /// The raw request body as a string.
    pub body: Option<String>,
    /// The parsed JSON body (if applicable).
    pub json_body: Option<Value>,
}

/// A response builder for route handlers.
pub struct Response {
    pub status_code: i64,
    pub body: Value,
    pub headers: HashMap<String, String>,
}

impl Response {
    /// Create a JSON response with status 200.
    pub fn json(body: Value) -> Self {
        Self {
            status_code: 200,
            body,
            headers: HashMap::new(),
        }
    }

    /// Set the HTTP status code.
    pub fn with_status(mut self, code: i64) -> Self {
        self.status_code = code;
        self
    }

    /// Add a header to the response.
    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }
}

type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
type HandlerFn = Box<dyn Fn(Request) -> BoxFuture<Result<Response, Error>> + Send + Sync>;

struct Route {
    path_pattern: String,
    methods: Vec<String>,
    handler: HandlerFn,
    segments: Vec<Segment>,
}

#[derive(Clone)]
enum Segment {
    Literal(String),
    Param(String),
}

fn compile_path(pattern: &str) -> Vec<Segment> {
    pattern
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s.starts_with('{') && s.ends_with('}') {
                Segment::Param(s[1..s.len() - 1].to_string())
            } else {
                Segment::Literal(s.to_string())
            }
        })
        .collect()
}

fn match_path(segments: &[Segment], path: &str) -> Option<HashMap<String, String>> {
    let path_parts: Vec<&str> = path
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    if segments.is_empty() && path_parts.is_empty() {
        return Some(HashMap::new());
    }
    if segments.len() != path_parts.len() {
        return None;
    }

    let mut params = HashMap::new();
    for (seg, part) in segments.iter().zip(path_parts.iter()) {
        match seg {
            Segment::Literal(lit) => {
                if lit != part {
                    return None;
                }
            }
            Segment::Param(name) => {
                params.insert(name.clone(), part.to_string());
            }
        }
    }
    Some(params)
}

/// The main application struct for the Choko framework.
pub struct Choko {
    app_name: String,
    routes: Vec<Route>,
}

impl Choko {
    /// Create a new Choko application.
    pub fn new(app_name: impl Into<String>) -> Self {
        Self {
            app_name: app_name.into(),
            routes: Vec::new(),
        }
    }

    /// Register a route with the given path pattern, HTTP methods, and handler.
    ///
    /// # Example
    /// ```ignore
    /// app.route("/users/{user_id}", &["GET"], |req| async move {
    ///     let user_id = req.path_params.get("user_id").unwrap();
    ///     Ok(Response::json(serde_json::json!({"user_id": user_id})))
    /// });
    /// ```
    pub fn route<F, Fut>(&mut self, path: &str, methods: &[&str], handler: F)
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Response, Error>> + Send + 'static,
    {
        let segments = compile_path(path);
        let methods = methods.iter().map(|m| m.to_uppercase()).collect();
        self.routes.push(Route {
            path_pattern: path.to_string(),
            methods,
            handler: Box::new(move |req| Box::pin(handler(req))),
            segments,
        });
    }

    /// Run the application as an AWS Lambda handler.
    pub async fn run(self) -> Result<(), Error> {
        let app = std::sync::Arc::new(self);
        let func = service_fn(move |event: LambdaEvent<ApiGatewayProxyRequest>| {
            let app = app.clone();
            async move { app.dispatch(event.payload).await }
        });
        lambda_runtime::run(func).await?;
        Ok(())
    }

    async fn dispatch(
        &self,
        event: ApiGatewayProxyRequest,
    ) -> Result<ApiGatewayProxyResponse, Error> {
        let path = event.path.as_deref().unwrap_or("/");
        let method = event.http_method.as_str().to_uppercase();

        // Find matching route
        let mut path_matched = false;
        for route in &self.routes {
            if let Some(path_params) = match_path(&route.segments, path) {
                path_matched = true;
                if route.methods.contains(&method) {
                    let request = self.build_request(&event, path_params);
                    let response = (route.handler)(request).await?;
                    return Ok(self.build_apigw_response(response));
                }
            }
        }

        if path_matched {
            Ok(self.error_response(405, "Method Not Allowed"))
        } else {
            Ok(self.error_response(404, "Not Found"))
        }
    }

    fn build_request(
        &self,
        event: &ApiGatewayProxyRequest,
        path_params: HashMap<String, String>,
    ) -> Request {
        let query_params = event
            .query_string_parameters
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        let headers = event
            .headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        let body_str = event.body.clone();

        let json_body = body_str
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());

        Request {
            path_params,
            query_params,
            headers,
            body: body_str,
            json_body,
        }
    }

    fn build_apigw_response(&self, resp: Response) -> ApiGatewayProxyResponse {
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
        for (k, v) in &resp.headers {
            if let (Ok(name), Ok(val)) = (
                http::header::HeaderName::from_bytes(k.as_bytes()),
                http::HeaderValue::from_str(v),
            ) {
                headers.insert(name, val);
            }
        }

        ApiGatewayProxyResponse {
            status_code: resp.status_code,
            headers,
            body: Some(Body::Text(resp.body.to_string())),
            is_base64_encoded: Some(false),
            multi_value_headers: Default::default(),
        }
    }

    fn error_response(&self, status_code: i64, message: &str) -> ApiGatewayProxyResponse {
        let body = serde_json::json!({ "error": message });
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
        ApiGatewayProxyResponse {
            status_code,
            headers,
            body: Some(Body::Text(body.to_string())),
            is_base64_encoded: Some(false),
            multi_value_headers: Default::default(),
        }
    }
}
