use std::collections::BTreeMap;
use std::error::Error as _;
use std::net::SocketAddr;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Method, Url};
use serde_json::{Value, json};

use super::{Integration, Services, StepOutcome};
use crate::expr;
use crate::netguard::{NetguardError, Policy};

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 300;
const FORBIDDEN_HEADERS: &[&str] = &["host", "content-length", "transfer-encoding", "connection"];
const HIDDEN_RESPONSE_HEADERS: &[&str] = &["set-cookie", "set-cookie2"];

// Résout le nom puis écarte les adresses internes : le contrôle porte sur l'adresse réellement utilisée.
struct GuardedResolver(Policy);

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let policy = self.0.clone();
        Box::pin(async move {
            let resolved: Vec<SocketAddr> =
                tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
            let mut allowed = Vec::new();
            let mut refusal = None;
            for address in resolved {
                match policy.check_ip(address.ip()) {
                    Ok(()) => allowed.push(address),
                    Err(err) => refusal = Some(err),
                }
            }
            if allowed.is_empty() {
                let err: Box<dyn std::error::Error + Send + Sync> = Box::new(
                    refusal.unwrap_or(NetguardError::InvalidEntry(name.as_str().to_owned())),
                );
                return Err(err);
            }
            Ok(Box::new(allowed.into_iter()) as Addrs)
        })
    }
}

pub fn build_client(policy: &Policy) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .dns_resolver(std::sync::Arc::new(GuardedResolver(policy.clone())))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(Duration::from_secs(10))
        .user_agent("scaling-waddle")
        .build()
}

struct Request {
    integration: String,
    method: Method,
    path: String,
    headers: BTreeMap<String, String>,
    body: Option<Value>,
    timeout: Duration,
}

fn parse(with: &Value) -> Result<Request, String> {
    let integration = with
        .get("integration")
        .and_then(Value::as_str)
        .ok_or("`with.integration` est requis et doit être une chaîne")?
        .to_owned();
    let method = match with.get("method") {
        None => Method::GET,
        Some(value) => match value.as_str().map(str::to_ascii_uppercase).as_deref() {
            Some("GET") => Method::GET,
            Some("POST") => Method::POST,
            Some("PUT") => Method::PUT,
            Some("PATCH") => Method::PATCH,
            Some("DELETE") => Method::DELETE,
            _ => return Err("méthode HTTP non supportée".into()),
        },
    };
    let path = match with.get("path") {
        None => "/".to_owned(),
        Some(value) => value
            .as_str()
            .ok_or("`with.path` doit être une chaîne")?
            .to_owned(),
    };
    let mut headers = BTreeMap::new();
    if let Some(value) = with.get("headers") {
        let map = value
            .as_object()
            .ok_or("`with.headers` doit être un objet")?;
        for (name, value) in map {
            let text = value
                .as_str()
                .ok_or("les valeurs de `with.headers` doivent être des chaînes")?;
            headers.insert(name.to_ascii_lowercase(), text.to_owned());
        }
    }
    let timeout_secs = match with.get("timeout_secs") {
        None => DEFAULT_TIMEOUT_SECS,
        Some(value) => value
            .as_u64()
            .filter(|secs| (1..=MAX_TIMEOUT_SECS).contains(secs))
            .ok_or("`with.timeout_secs` doit être compris entre 1 et 300")?,
    };
    let body = with.get("body").filter(|body| !body.is_null()).cloned();
    Ok(Request {
        integration,
        method,
        path,
        headers,
        body,
        timeout: Duration::from_secs(timeout_secs),
    })
}

fn build_url(integration: &Integration, path: &str, policy: &Policy) -> Result<Url, String> {
    let base =
        Url::parse(&integration.base_url).map_err(|_| "URL de base de l'intégration invalide")?;
    if !matches!(base.scheme(), "http" | "https") {
        return Err("seuls http et https sont permis".into());
    }
    let clean_path = path.starts_with('/')
        && !path.starts_with("//")
        && !path.contains('\\')
        && !path.chars().any(char::is_control);
    if !clean_path {
        return Err("`with.path` doit commencer par un seul `/` et ne contenir ni `\\` ni caractère de contrôle".into());
    }
    let url = Url::parse(&format!(
        "{}{path}",
        integration.base_url.trim_end_matches('/')
    ))
    .map_err(|_| "chemin invalide")?;
    if url.host_str() != base.host_str()
        || url.port_or_known_default() != base.port_or_known_default()
    {
        return Err("le chemin ne peut pas changer d'hôte".into());
    }
    let host = url.host_str().ok_or("hôte manquant")?;
    policy.check_host(host).map_err(|err| err.to_string())?;
    Ok(url)
}

fn header_map(
    integration: &Integration,
    step: &BTreeMap<String, String>,
) -> Result<HeaderMap, String> {
    let mut merged = integration.headers.clone();
    merged.extend(step.clone());
    let mut map = HeaderMap::new();
    for (name, value) in merged {
        let lower = name.to_ascii_lowercase();
        if FORBIDDEN_HEADERS.contains(&lower.as_str()) {
            return Err(format!("en-tête interdit: {name}"));
        }
        let name = HeaderName::from_bytes(lower.as_bytes())
            .map_err(|_| format!("nom d'en-tête invalide: {name}"))?;
        let value = HeaderValue::from_str(&value)
            .map_err(|_| format!("valeur d'en-tête invalide pour {name}"))?;
        map.insert(name, value);
    }
    Ok(map)
}

pub async fn execute(with: &Value, vars: &Value, services: &Services) -> StepOutcome {
    let rendered = match expr::render(with, vars) {
        Ok(rendered) => rendered,
        Err(err) => return StepOutcome::fatal(err.to_string()),
    };
    let request = match parse(&rendered) {
        Ok(request) => request,
        Err(err) => return StepOutcome::fatal(err),
    };
    if let Some(reason) = services.unavailable_reason(&request.integration) {
        return StepOutcome::fatal(format!(
            "intégration {} indisponible: {reason}",
            request.integration
        ));
    }
    let Some(integration) = services.integrations.get(&request.integration) else {
        return StepOutcome::fatal(format!("intégration inconnue: {}", request.integration));
    };
    let url = match build_url(integration, &request.path, &services.policy) {
        Ok(url) => url,
        Err(err) => return StepOutcome::fatal(err),
    };
    let mut headers = match header_map(integration, &request.headers) {
        Ok(headers) => headers,
        Err(err) => return StepOutcome::fatal(err),
    };
    let Some(client) = &services.client else {
        return StepOutcome::fatal("client HTTP indisponible");
    };

    let mut builder = client.request(request.method, url).timeout(request.timeout);
    if let Some(body) = &request.body {
        headers
            .entry(CONTENT_TYPE)
            .or_insert(HeaderValue::from_static("application/json"));
        builder = builder.body(body.to_string());
    }
    let response = match builder.headers(headers).send().await {
        Ok(response) => response,
        Err(err) => return transport_failure(&err),
    };
    read_response(response).await
}

fn transport_failure(err: &reqwest::Error) -> StepOutcome {
    let mut chain = err.to_string();
    let mut source = err.source();
    while let Some(next) = source {
        chain.push_str(": ");
        chain.push_str(&next.to_string());
        source = next.source();
    }
    let refused = chain.contains("refusée");
    StepOutcome::Failed {
        error: format!("requête HTTP échouée: {chain}"),
        retryable: !refused,
    }
}

async fn read_response(mut response: reqwest::Response) -> StepOutcome {
    let status = response.status();
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    for (name, value) in response.headers() {
        if HIDDEN_RESPONSE_HEADERS.contains(&name.as_str()) {
            continue;
        }
        if let Ok(text) = value.to_str() {
            headers
                .entry(name.as_str().to_owned())
                .and_modify(|existing| {
                    existing.push_str(", ");
                    existing.push_str(text);
                })
                .or_insert_with(|| text.to_owned());
        }
    }
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                body.extend_from_slice(&chunk);
                if body.len() > MAX_RESPONSE_BYTES {
                    return StepOutcome::fatal("réponse trop grande (plafond de 2 Mo)");
                }
            }
            Ok(None) => break,
            Err(err) => return transport_failure(&err),
        }
    }
    if !status.is_success() {
        let retryable = status.is_server_error() || matches!(status.as_u16(), 408 | 429);
        return StepOutcome::Failed {
            error: format!("HTTP {}", status.as_u16()),
            retryable,
        };
    }
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into_owned()))
    };
    StepOutcome::Succeeded(
        json!({ "status_code": status.as_u16(), "headers": headers, "body": parsed }),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use serde_json::{Value, json};
    use wiremock::matchers::{body_json, header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::netguard::Policy;
    use crate::steps::{Integration, Services, StepOutcome, execute};

    fn vars() -> Value {
        json!({ "event": { "payload": { "service": "api", "version": "1.2.3" } }, "steps": {} })
    }

    fn services(server: &MockServer, host: &str, headers: &[(&str, &str)]) -> Services {
        let port = server.address().port();
        let integration = Integration {
            base_url: format!("http://{host}:{port}"),
            headers: headers
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        };
        Services::new(
            Policy::parse_allowed("127.0.0.0/8").unwrap(),
            BTreeMap::from([("hook".to_owned(), integration)]),
        )
    }

    async fn run(services: &Services, with: Value) -> StepOutcome {
        execute("http.request", &with, &vars(), services).await
    }

    fn failure(outcome: &StepOutcome) -> (&str, bool) {
        match outcome {
            StepOutcome::Failed { error, retryable } => (error.as_str(), *retryable),
            StepOutcome::Succeeded(output) => panic!("succès inattendu: {output}"),
        }
    }

    fn output(outcome: StepOutcome) -> Value {
        match outcome {
            StepOutcome::Succeeded(output) => output,
            StepOutcome::Failed { error, .. } => panic!("échec inattendu: {error}"),
        }
    }

    #[tokio::test]
    async fn get_returns_status_headers_and_parsed_json() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/status"))
            .and(query_param("q", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
            .mount(&server)
            .await;
        let services = services(&server, "127.0.0.1", &[]);

        let out = output(
            run(
                &services,
                json!({ "integration": "hook", "path": "/status?q=1" }),
            )
            .await,
        );

        assert_eq!(out["status_code"], 200);
        assert_eq!(out["body"], json!({ "ok": true }));
        assert_eq!(out["headers"]["content-type"], "application/json");
    }

    #[tokio::test]
    async fn post_sends_a_templated_json_body_and_merged_headers() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/deployments"))
            .and(header("x-team", "platform"))
            .and(header("x-step", "yes"))
            .and(body_json(json!({ "service": "api", "version": "1.2.3" })))
            .respond_with(ResponseTemplate::new(201).set_body_string("created"))
            .expect(1)
            .mount(&server)
            .await;
        let services = services(
            &server,
            "127.0.0.1",
            &[("x-team", "platform"), ("x-step", "no")],
        );

        let out = output(
            run(
                &services,
                json!({
                    "integration": "hook",
                    "method": "POST",
                    "path": "/deployments",
                    "headers": { "x-step": "yes" },
                    "body": { "service": "${{ event.payload.service }}", "version": "${{ event.payload.version }}" }
                }),
            )
            .await,
        );

        assert_eq!(out["status_code"], 201);
        assert_eq!(out["body"], "created");
    }

    #[tokio::test]
    async fn server_errors_and_rate_limits_are_retryable_client_errors_are_not() {
        let server = MockServer::start().await;
        for (route, status) in [
            ("/a", 500),
            ("/b", 503),
            ("/c", 429),
            ("/d", 408),
            ("/e", 404),
            ("/f", 400),
        ] {
            Mock::given(path(route))
                .respond_with(ResponseTemplate::new(status))
                .mount(&server)
                .await;
        }
        let services = services(&server, "127.0.0.1", &[]);
        for (route, retryable) in [
            ("/a", true),
            ("/b", true),
            ("/c", true),
            ("/d", true),
            ("/e", false),
            ("/f", false),
        ] {
            let outcome = run(&services, json!({ "integration": "hook", "path": route })).await;
            assert_eq!(failure(&outcome).1, retryable, "{route}");
        }
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let server = MockServer::start().await;
        let target = MockServer::start().await;
        Mock::given(path("/go"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("{}/secret", target.uri())),
            )
            .mount(&server)
            .await;
        Mock::given(path("/secret"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&target)
            .await;
        let services = services(&server, "127.0.0.1", &[]);

        let outcome = run(&services, json!({ "integration": "hook", "path": "/go" })).await;

        assert!(!failure(&outcome).1);
    }

    #[tokio::test]
    async fn unknown_integration_method_or_missing_field_fail_without_retry() {
        let server = MockServer::start().await;
        let services = services(&server, "127.0.0.1", &[]);
        for with in [
            json!({ "integration": "inconnue" }),
            json!({}),
            json!({ "integration": 3 }),
            json!({ "integration": "hook", "method": "TRACE" }),
            json!({ "integration": "hook", "headers": { "x": 1 } }),
            json!({ "integration": "hook", "timeout_secs": 0 }),
            json!({ "integration": "hook", "body": "${{ nope.field }}" }),
        ] {
            let outcome = run(&services, with.clone()).await;
            assert!(!failure(&outcome).1, "{with}");
        }
    }

    #[tokio::test]
    async fn paths_cannot_escape_the_integration_host() {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let services = services(&server, "127.0.0.1", &[]);
        for bad in [
            "//evil.com/x",
            "http://evil.com/x",
            "no-slash",
            "/a\\b",
            "@evil.com",
            "/ok\r\nx: y",
        ] {
            let outcome = run(&services, json!({ "integration": "hook", "path": bad })).await;
            assert!(!failure(&outcome).1, "{bad}");
        }
    }

    #[tokio::test]
    async fn internal_ip_literals_are_refused_before_any_request() {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let port = server.address().port();
        let integration = Integration {
            base_url: format!("http://127.0.0.1:{port}"),
            headers: BTreeMap::new(),
        };
        let services = Services::new(
            Policy::default(),
            BTreeMap::from([("hook".to_owned(), integration)]),
        );

        let outcome = run(&services, json!({ "integration": "hook" })).await;

        let (error, retryable) = failure(&outcome);
        assert!(error.contains("refusée"), "{error}");
        assert!(!retryable);
    }

    #[tokio::test]
    async fn hostnames_resolving_to_internal_addresses_are_refused_at_connect_time() {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&server)
            .await;
        let port = server.address().port();
        let integration = Integration {
            base_url: format!("http://localhost:{port}"),
            headers: BTreeMap::new(),
        };
        let services = Services::new(
            Policy::default(),
            BTreeMap::from([("hook".to_owned(), integration)]),
        );

        let outcome = run(&services, json!({ "integration": "hook" })).await;

        let (error, retryable) = failure(&outcome);
        assert!(error.contains("refusée"), "{error}");
        assert!(!retryable);
    }

    #[tokio::test]
    async fn oversized_responses_are_cut_off() {
        let server = MockServer::start().await;
        Mock::given(path("/big"))
            .respond_with(
                ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 2 * 1024 * 1024 + 10]),
            )
            .mount(&server)
            .await;
        let services = services(&server, "127.0.0.1", &[]);

        let outcome = run(&services, json!({ "integration": "hook", "path": "/big" })).await;

        let (error, retryable) = failure(&outcome);
        assert!(error.contains("trop grande"), "{error}");
        assert!(!retryable);
    }

    #[tokio::test]
    async fn timeouts_are_retryable() {
        let server = MockServer::start().await;
        Mock::given(path("/slow"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_millis(1800)))
            .mount(&server)
            .await;
        let services = services(&server, "127.0.0.1", &[]);

        let outcome = run(
            &services,
            json!({ "integration": "hook", "path": "/slow", "timeout_secs": 1 }),
        )
        .await;

        assert!(failure(&outcome).1);
    }

    #[tokio::test]
    async fn connection_refused_is_retryable() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let integration = Integration {
            base_url: format!("http://127.0.0.1:{port}"),
            headers: BTreeMap::new(),
        };
        let services = Services::new(
            Policy::parse_allowed("127.0.0.0/8").unwrap(),
            BTreeMap::from([("hook".to_owned(), integration)]),
        );

        let outcome = run(&services, json!({ "integration": "hook" })).await;

        assert!(failure(&outcome).1);
    }

    #[tokio::test]
    async fn set_cookie_is_not_exposed_in_the_output() {
        let server = MockServer::start().await;
        Mock::given(path("/c"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("set-cookie", "sid=secret")
                    .insert_header("x-ok", "1"),
            )
            .mount(&server)
            .await;
        let services = services(&server, "127.0.0.1", &[]);

        let out = output(run(&services, json!({ "integration": "hook", "path": "/c" })).await);

        assert!(out["headers"].get("set-cookie").is_none());
        assert_eq!(out["headers"]["x-ok"], "1");
    }

    #[tokio::test]
    async fn an_unavailable_integration_fails_without_any_request() {
        let server = MockServer::start().await;
        let services =
            services(&server, "127.0.0.1", &[]).mark_unavailable("hook", "secret inconnu: x");
        let outcome = run(&services, json!({ "integration": "hook", "path": "/go" })).await;
        let (error, retryable) = failure(&outcome);
        assert!(error.contains("indisponible") && error.contains("secret inconnu: x"));
        assert!(!retryable);
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
