use aws_lambda_events::encodings::Body;
use aws_lambda_events::event::apigw::{ApiGatewayProxyRequest, ApiGatewayProxyResponse};
pub use lambda_runtime::Error;
use lambda_runtime::{service_fn, LambdaEvent};
pub use serde_json;
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
    _path_pattern: String,
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
    _app_name: String,
    routes: Vec<Route>,
}

impl Choko {
    /// Create a new Choko application.
    pub fn new(app_name: impl Into<String>) -> Self {
        Self {
            _app_name: app_name.into(),
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
            _path_pattern: path.to_string(),
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
                    return match (route.handler)(request).await {
                        Ok(response) => Ok(self.build_apigw_response(response)),
                        Err(e) => {
                            Ok(self.error_response(500, &format!("Internal Server Error: {e}")))
                        }
                    };
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

        let mut r = ApiGatewayProxyResponse::default();
        r.status_code = resp.status_code;
        r.headers = headers;
        r.body = Some(Body::Text(resp.body.to_string()));
        r
    }

    fn error_response(&self, status_code: i64, message: &str) -> ApiGatewayProxyResponse {
        let body = serde_json::json!({ "error": message });
        let mut headers = http::HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
        let mut r = ApiGatewayProxyResponse::default();
        r.status_code = status_code;
        r.headers = headers;
        r.body = Some(Body::Text(body.to_string()));
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- compile_path tests ---

    #[test]
    fn compile_path_root() {
        let segments = compile_path("/");
        assert!(segments.is_empty());
    }

    #[test]
    fn compile_path_literal() {
        let segments = compile_path("/users/list");
        assert_eq!(segments.len(), 2);
        assert!(matches!(&segments[0], Segment::Literal(s) if s == "users"));
        assert!(matches!(&segments[1], Segment::Literal(s) if s == "list"));
    }

    #[test]
    fn compile_path_with_param() {
        let segments = compile_path("/users/{user_id}/posts/{post_id}");
        assert_eq!(segments.len(), 4);
        assert!(matches!(&segments[0], Segment::Literal(s) if s == "users"));
        assert!(matches!(&segments[1], Segment::Param(s) if s == "user_id"));
        assert!(matches!(&segments[2], Segment::Literal(s) if s == "posts"));
        assert!(matches!(&segments[3], Segment::Param(s) if s == "post_id"));
    }

    // --- match_path tests ---

    #[test]
    fn match_root_path() {
        let segments = compile_path("/");
        assert!(match_path(&segments, "/").is_some());
        assert!(match_path(&segments, "").is_some());
    }

    #[test]
    fn match_literal_path() {
        let segments = compile_path("/users/list");
        assert!(match_path(&segments, "/users/list").is_some());
        assert!(match_path(&segments, "/users/other").is_none());
        assert!(match_path(&segments, "/users").is_none());
        assert!(match_path(&segments, "/users/list/extra").is_none());
    }

    #[test]
    fn match_path_extracts_single_param() {
        let segments = compile_path("/users/{user_id}");
        let params = match_path(&segments, "/users/42").unwrap();
        assert_eq!(params.get("user_id").unwrap(), "42");
    }

    #[test]
    fn match_path_extracts_multiple_params() {
        let segments = compile_path("/users/{user_id}/posts/{post_id}");
        let params = match_path(&segments, "/users/7/posts/99").unwrap();
        assert_eq!(params.get("user_id").unwrap(), "7");
        assert_eq!(params.get("post_id").unwrap(), "99");
    }

    #[test]
    fn match_path_rejects_wrong_length() {
        let segments = compile_path("/users/{id}");
        assert!(match_path(&segments, "/users").is_none());
        assert!(match_path(&segments, "/users/1/extra").is_none());
    }

    #[test]
    fn match_path_rejects_wrong_literal() {
        let segments = compile_path("/api/users");
        assert!(match_path(&segments, "/api/posts").is_none());
    }

    // --- Response builder tests ---

    #[test]
    fn response_json_defaults_to_200() {
        let resp = Response::json(json!({"ok": true}));
        assert_eq!(resp.status_code, 200);
        assert_eq!(resp.body, json!({"ok": true}));
        assert!(resp.headers.is_empty());
    }

    #[test]
    fn response_with_status() {
        let resp = Response::json(json!(null)).with_status(404);
        assert_eq!(resp.status_code, 404);
    }

    #[test]
    fn response_with_header() {
        let resp = Response::json(json!(null))
            .with_header("X-Custom", "value1")
            .with_header("X-Other", "value2");
        assert_eq!(resp.headers.get("X-Custom").unwrap(), "value1");
        assert_eq!(resp.headers.get("X-Other").unwrap(), "value2");
    }

    // --- dispatch integration tests ---

    fn make_apigw_request(
        method: &str,
        path: &str,
        body: Option<String>,
    ) -> ApiGatewayProxyRequest {
        let mut req = ApiGatewayProxyRequest::default();
        req.http_method = method.parse().unwrap();
        req.path = Some(path.to_string());
        req.body = body;
        req
    }

    #[tokio::test]
    async fn dispatch_matches_get_root() {
        let mut app = Choko::new("test");
        app.route("/", &["GET"], |_req| async {
            Ok(Response::json(json!({"hello": "world"})))
        });

        let event = make_apigw_request("GET", "/", None);
        let resp = app.dispatch(event).await.unwrap();

        assert_eq!(resp.status_code, 200);
        let body: Value = serde_json::from_str(match resp.body.as_ref().unwrap() {
            Body::Text(s) => s,
            _ => panic!("expected text body"),
        })
        .unwrap();
        assert_eq!(body, json!({"hello": "world"}));
    }

    #[tokio::test]
    async fn dispatch_extracts_path_params() {
        let mut app = Choko::new("test");
        app.route("/users/{user_id}", &["GET"], |req| async move {
            let uid = req.path_params.get("user_id").cloned().unwrap_or_default();
            Ok(Response::json(json!({"user_id": uid})))
        });

        let event = make_apigw_request("GET", "/users/123", None);
        let resp = app.dispatch(event).await.unwrap();

        assert_eq!(resp.status_code, 200);
        let body: Value = serde_json::from_str(match resp.body.as_ref().unwrap() {
            Body::Text(s) => s,
            _ => panic!("expected text body"),
        })
        .unwrap();
        assert_eq!(body["user_id"], "123");
    }

    #[tokio::test]
    async fn dispatch_returns_404_for_unknown_path() {
        let mut app = Choko::new("test");
        app.route("/", &["GET"], |_req| async {
            Ok(Response::json(json!({"ok": true})))
        });

        let event = make_apigw_request("GET", "/unknown", None);
        let resp = app.dispatch(event).await.unwrap();

        assert_eq!(resp.status_code, 404);
    }

    #[tokio::test]
    async fn dispatch_returns_405_for_wrong_method() {
        let mut app = Choko::new("test");
        app.route("/users", &["POST"], |_req| async {
            Ok(Response::json(json!({"created": true})))
        });

        let event = make_apigw_request("GET", "/users", None);
        let resp = app.dispatch(event).await.unwrap();

        assert_eq!(resp.status_code, 405);
    }

    #[tokio::test]
    async fn dispatch_with_json_body() {
        let mut app = Choko::new("test");
        app.route("/items", &["POST"], |req| async move {
            let name = req
                .json_body
                .as_ref()
                .and_then(|v| v.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            Ok(Response::json(json!({"received": name})).with_status(201))
        });

        let body = json!({"name": "test-item"}).to_string();
        let event = make_apigw_request("POST", "/items", Some(body));
        let resp = app.dispatch(event).await.unwrap();

        assert_eq!(resp.status_code, 201);
        let body: Value = serde_json::from_str(match resp.body.as_ref().unwrap() {
            Body::Text(s) => s,
            _ => panic!("expected text body"),
        })
        .unwrap();
        assert_eq!(body["received"], "test-item");
    }

    #[tokio::test]
    async fn dispatch_multiple_routes() {
        let mut app = Choko::new("test");
        app.route("/", &["GET"], |_req| async {
            Ok(Response::json(json!({"route": "index"})))
        });
        app.route("/users", &["GET"], |_req| async {
            Ok(Response::json(json!({"route": "users_list"})))
        });
        app.route("/users/{id}", &["GET"], |_req| async {
            Ok(Response::json(json!({"route": "user_detail"})))
        });

        let extract_route = |resp: ApiGatewayProxyResponse| -> String {
            let body: Value = serde_json::from_str(match resp.body.as_ref().unwrap() {
                Body::Text(s) => s,
                _ => panic!("expected text body"),
            })
            .unwrap();
            body["route"].as_str().unwrap().to_string()
        };

        let resp = app
            .dispatch(make_apigw_request("GET", "/", None))
            .await
            .unwrap();
        assert_eq!(extract_route(resp), "index");

        let resp = app
            .dispatch(make_apigw_request("GET", "/users", None))
            .await
            .unwrap();
        assert_eq!(extract_route(resp), "users_list");

        let resp = app
            .dispatch(make_apigw_request("GET", "/users/5", None))
            .await
            .unwrap();
        assert_eq!(extract_route(resp), "user_detail");
    }

    #[tokio::test]
    async fn dispatch_response_has_content_type_header() {
        let mut app = Choko::new("test");
        app.route("/", &["GET"], |_req| async {
            Ok(Response::json(json!({})))
        });

        let resp = app
            .dispatch(make_apigw_request("GET", "/", None))
            .await
            .unwrap();
        let ct = resp.headers.get(http::header::CONTENT_TYPE).unwrap();
        assert_eq!(ct, "application/json");
    }

    #[tokio::test]
    async fn dispatch_custom_response_headers() {
        let mut app = Choko::new("test");
        app.route("/", &["GET"], |_req| async {
            Ok(Response::json(json!({})).with_header("x-request-id", "abc-123"))
        });

        let resp = app
            .dispatch(make_apigw_request("GET", "/", None))
            .await
            .unwrap();
        let val = resp.headers.get("x-request-id").unwrap();
        assert_eq!(val, "abc-123");
    }

    #[tokio::test]
    async fn dispatch_returns_500_on_handler_error() {
        let mut app = Choko::new("test");
        app.route("/fail", &["GET"], |_req| async {
            Err::<Response, Error>("something went wrong".into())
        });

        let resp = app
            .dispatch(make_apigw_request("GET", "/fail", None))
            .await
            .unwrap();

        assert_eq!(resp.status_code, 500);
        let body: Value = serde_json::from_str(match resp.body.as_ref().unwrap() {
            Body::Text(s) => s,
            _ => panic!("expected text body"),
        })
        .unwrap();
        assert!(body["error"]
            .as_str()
            .unwrap()
            .contains("something went wrong"));
    }
}
