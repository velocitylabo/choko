use choko::serde_json::json;
use choko::{Choko, Error, Request, Response};

async fn index(_req: Request) -> Result<Response, Error> {
    Ok(Response::json(json!({"message": "Hello from Choko!"})))
}

async fn get_user(req: Request) -> Result<Response, Error> {
    let user_id = req.path_params.get("user_id").unwrap();
    Ok(Response::json(json!({"user_id": user_id})))
}

async fn create_user(req: Request) -> Result<Response, Error> {
    let body = req.json_body.unwrap_or(json!({}));
    Ok(Response::json(json!({"created": body})).with_status(201))
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let mut app = Choko::new("choko-app");

    app.route("/", &["GET"], index);
    app.route("/users/{user_id}", &["GET"], get_user);
    app.route("/users", &["POST"], create_user);

    app.run().await
}
