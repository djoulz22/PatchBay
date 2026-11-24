use crate::{audio::FrameQueue, graph::valid_tab_id};
use http_body_util::{BodyExt, Empty, Limited};
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Body, Bytes},
    header::{self, HeaderName, HeaderValue},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    convert::Infallible,
    error::Error,
    fmt::Write,
    net::{Ipv4Addr, TcpListener},
    sync::{Arc, Mutex, MutexGuard, Weak},
    thread,
    time::{Duration, Instant},
};
use tokio::{
    sync::{Semaphore, oneshot},
    time::timeout,
};

const EXTENSION_ORIGIN: &str = "chrome-extension://anfbabjdfjjifekgbnonaglhojjlkefl";
const MAX_BODY: usize = 16_384;
const MAX_ANNOUNCEMENT: usize = 2_048;
const MAX_TABS: usize = 64;
const MAX_CONNECTIONS: usize = 32;
const TAB_IDLE: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Serialize)]
pub struct BrowserPairing {
    port: u16,
    token: String,
}

#[derive(Clone, Serialize)]
pub struct BrowserTab {
    id: String,
    title: String,
    browser: BrowserName,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
enum BrowserName {
    Brave,
    Chrome,
    Edge,
    Chromium,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TabAnnouncement {
    title: String,
    browser: BrowserName,
}

struct ActiveTab {
    tab: BrowserTab,
    last_seen: Instant,
}

#[derive(Default)]
struct Registry {
    active: HashMap<String, ActiveTab>,
    // The audio runtime owns the reader. Registered tabs with no reader have no queue.
    queues: HashMap<String, Weak<FrameQueue>>,
}

impl Registry {
    fn prune(&mut self, now: Instant) {
        self.active
            .retain(|_, entry| now.duration_since(entry.last_seen) < TAB_IDLE);
        self.queues.retain(|_, queue| queue.strong_count() > 0);
    }

    fn register(&mut self, id: &str, tab: TabAnnouncement) -> StatusCode {
        let now = Instant::now();
        self.prune(now);
        if self.active.len() >= MAX_TABS && !self.active.contains_key(id) {
            return StatusCode::TOO_MANY_REQUESTS;
        }
        self.active.insert(
            id.to_owned(),
            ActiveTab {
                tab: BrowserTab {
                    id: id.to_owned(),
                    title: tab.title,
                    browser: tab.browser,
                },
                last_seen: now,
            },
        );
        StatusCode::NO_CONTENT
    }

    fn feed(&mut self, id: &str, pcm: &[u8]) -> StatusCode {
        let now = Instant::now();
        let Some(tab) = self.active.get_mut(id) else {
            return StatusCode::NOT_FOUND;
        };
        if now.duration_since(tab.last_seen) >= TAB_IDLE {
            self.active.remove(id);
            return StatusCode::NOT_FOUND;
        }
        tab.last_seen = now;
        if let Some(queue) = self.queues.get(id).and_then(Weak::upgrade) {
            // The registry lock serializes all producers; frames are interleaved stereo f32.
            for frame in pcm.chunks_exact(8) {
                queue.push(|channel| {
                    let start = channel * 4;
                    let sample = f32::from_le_bytes(frame[start..start + 4].try_into().unwrap());
                    if sample.is_finite() {
                        sample.clamp(-16.0, 16.0)
                    } else {
                        0.0
                    }
                });
            }
        }
        StatusCode::NO_CONTENT
    }
}

#[derive(Clone, Default)]
pub struct BrowserCapture {
    registry: Arc<Mutex<Registry>>,
    server: Arc<Mutex<Option<BrowserServer>>>,
}

impl BrowserCapture {
    pub fn pairing(&self) -> Result<BrowserPairing, String> {
        let mut server = lock(&self.server);
        if let Some(running) = server
            .as_ref()
            .filter(|running| !running.shutdown.is_closed())
        {
            return Ok(running.pairing.clone());
        }
        let running = BrowserServer::start(self.registry.clone())?;
        let pairing = running.pairing.clone();
        *server = Some(running);
        Ok(pairing)
    }

    pub fn tabs(&self) -> Vec<BrowserTab> {
        let mut registry = lock(&self.registry);
        registry.prune(Instant::now());
        let mut tabs: Vec<_> = registry
            .active
            .values()
            .map(|entry| entry.tab.clone())
            .collect();
        tabs.sort_by(|a, b| a.title.cmp(&b.title).then(a.id.cmp(&b.id)));
        tabs
    }

    pub fn queue(&self, id: &str) -> Arc<FrameQueue> {
        let mut registry = lock(&self.registry);
        registry.prune(Instant::now());
        if let Some(queue) = registry.queues.get(id).and_then(Weak::upgrade) {
            return queue;
        }
        let queue = FrameQueue::new(2);
        registry
            .queues
            .insert(id.to_owned(), Arc::downgrade(&queue));
        queue
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct BrowserServer {
    pairing: BrowserPairing,
    shutdown: oneshot::Sender<()>,
}

impl BrowserServer {
    fn start(registry: Arc<Mutex<Registry>>) -> Result<Self, String> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(pairing_error)?;
        let port = listener.local_addr().map_err(pairing_error)?.port();
        listener.set_nonblocking(true).map_err(pairing_error)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(pairing_error)?;
        let listener = {
            let _enter = runtime.enter();
            tokio::net::TcpListener::from_std(listener).map_err(pairing_error)?
        };
        let mut randomness = [0_u8; 16];
        getrandom::fill(&mut randomness).map_err(pairing_error)?;
        let mut token = String::with_capacity(32);
        for byte in randomness {
            write!(token, "{byte:02x}").unwrap();
        }
        let access = Arc::new(Access {
            host: format!("127.0.0.1:{port}"),
            authorization: format!("Bearer {token}"),
        });
        let (shutdown, receiver) = oneshot::channel();
        thread::Builder::new()
            .name("patchbay-browser-audio".into())
            .spawn(move || runtime.block_on(serve(listener, registry, access, receiver)))
            .map_err(pairing_error)?;
        Ok(Self {
            pairing: BrowserPairing { port, token },
            shutdown,
        })
    }
}

fn pairing_error(error: impl std::fmt::Display) -> String {
    format!("Association du navigateur indisponible ({error}).")
}

struct Access {
    host: String,
    authorization: String,
}

async fn serve(
    listener: tokio::net::TcpListener,
    registry: Arc<Mutex<Registry>>,
    access: Arc<Access>,
    mut shutdown: oneshot::Receiver<()>,
) {
    let available = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let incoming = tokio::select! {
            _ = &mut shutdown => return,
            accept = listener.accept() => accept,
        };
        let Ok((stream, peer)) = incoming else { return };
        if !peer.ip().is_loopback() {
            continue;
        }
        let Ok(permit) = available.clone().try_acquire_owned() else {
            continue;
        };
        let registry = registry.clone();
        let access = access.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _ = stream.set_nodelay(true);
            let service = service_fn(move |request| {
                let registry = registry.clone();
                let access = access.clone();
                async move { Ok::<_, Infallible>(handle(&registry, &access, request).await) }
            });
            let _ = http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(REQUEST_TIMEOUT)
                .max_headers(32)
                .max_buf_size(32_768)
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}

enum Route<'a> {
    Pair,
    Start(&'a str),
    Pcm(&'a str),
    Stop(&'a str),
}

impl<'a> Route<'a> {
    fn parse(path: &'a str) -> Option<Self> {
        if path == "/v1/pairing" {
            return Some(Self::Pair);
        }
        let path = path.strip_prefix("/v1/tabs/")?;
        let (id, route) = match path.split_once('/') {
            Some((id, "start")) => (id, Self::Start(id)),
            Some((id, "pcm")) => (id, Self::Pcm(id)),
            Some(_) => return None,
            None => (path, Self::Stop(path)),
        };
        valid_tab_id(id).then_some(route)
    }

    fn method(&self) -> Method {
        if matches!(self, Self::Stop(_)) {
            Method::DELETE
        } else {
            Method::POST
        }
    }
}

type Reply = Response<Empty<Bytes>>;

fn reply(status: StatusCode, cors: bool) -> Reply {
    let mut reply = Response::new(Empty::new());
    *reply.status_mut() = status;
    reply
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if status.is_client_error() {
        reply
            .headers_mut()
            .insert(header::CONNECTION, HeaderValue::from_static("close"));
    }
    if cors {
        let headers = reply.headers_mut();
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static(EXTENSION_ORIGIN),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("Authorization, Content-Type"),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("POST, DELETE"),
        );
        headers.insert(
            HeaderName::from_static("access-control-allow-private-network"),
            HeaderValue::from_static("true"),
        );
    }
    reply
}

async fn handle<B>(registry: &Mutex<Registry>, access: &Access, request: Request<B>) -> Reply
where
    B: Body<Data = Bytes>,
    B::Error: Into<Box<dyn Error + Send + Sync>>,
{
    let (parts, body) = request.into_parts();
    let header = |name| {
        parts
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
    };
    if header(header::ORIGIN) != Some(EXTENSION_ORIGIN)
        || header(header::HOST) != Some(access.host.as_str())
    {
        return reply(StatusCode::FORBIDDEN, false);
    }
    let Some(route) = Route::parse(parts.uri.path()).filter(|_| parts.uri.query().is_none()) else {
        return reply(StatusCode::NOT_FOUND, true);
    };
    let method = route.method();
    if parts.method == Method::OPTIONS {
        return reply(
            if header(header::ACCESS_CONTROL_REQUEST_METHOD) == Some(method.as_str()) {
                StatusCode::NO_CONTENT
            } else {
                StatusCode::METHOD_NOT_ALLOWED
            },
            true,
        );
    }
    if !header(header::AUTHORIZATION).is_some_and(|value| authorized(value, &access.authorization))
    {
        return reply(StatusCode::UNAUTHORIZED, true);
    }
    if parts.method != method {
        return reply(StatusCode::METHOD_NOT_ALLOWED, true);
    }
    let status = match route {
        Route::Pair => match read_body(body, 0).await {
            Ok(_) => StatusCode::NO_CONTENT,
            Err(status) => status,
        },
        Route::Stop(id) => {
            if let Err(status) = read_body(body, 0).await {
                return reply(status, true);
            }
            lock(registry).active.remove(id);
            StatusCode::NO_CONTENT
        }
        Route::Start(id) => {
            if header(header::CONTENT_TYPE) != Some("application/json") {
                return reply(StatusCode::UNSUPPORTED_MEDIA_TYPE, true);
            }
            let bytes = match read_body(body, MAX_ANNOUNCEMENT).await {
                Ok(bytes) => bytes,
                Err(status) => return reply(status, true),
            };
            let Ok(tab) = serde_json::from_slice::<TabAnnouncement>(&bytes) else {
                return reply(StatusCode::BAD_REQUEST, true);
            };
            if tab.title.trim().is_empty()
                || tab.title.chars().count() > 256
                || tab.title.chars().any(char::is_control)
            {
                return reply(StatusCode::BAD_REQUEST, true);
            }
            lock(registry).register(id, tab)
        }
        Route::Pcm(id) => {
            if header(header::CONTENT_TYPE) != Some("application/octet-stream") {
                return reply(StatusCode::UNSUPPORTED_MEDIA_TYPE, true);
            }
            let pcm = match read_body(body, MAX_BODY).await {
                Ok(pcm) => pcm,
                Err(status) => return reply(status, true),
            };
            if pcm.is_empty() || !pcm.len().is_multiple_of(8) {
                return reply(StatusCode::BAD_REQUEST, true);
            }
            lock(registry).feed(id, &pcm)
        }
    };
    reply(status, true)
}

fn authorized(provided: &str, expected: &str) -> bool {
    provided.len() == expected.len()
        && provided
            .bytes()
            .zip(expected.bytes())
            .fold(0_u8, |difference, (left, right)| {
                difference | (left ^ right)
            })
            == 0
}

async fn read_body<B>(body: B, limit: usize) -> Result<Bytes, StatusCode>
where
    B: Body<Data = Bytes>,
    B::Error: Into<Box<dyn Error + Send + Sync>>,
{
    match timeout(REQUEST_TIMEOUT, Limited::new(body, limit).collect()).await {
        Ok(Ok(body)) => Ok(body.to_bytes()),
        Ok(Err(_)) => Err(StatusCode::PAYLOAD_TOO_LARGE),
        Err(_) => Err(StatusCode::REQUEST_TIMEOUT),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::Full;

    const TAB_ID: &str = "04e54449-28a1-4eea-8fec-9fb4427eed82";
    const TITLE: &str = r#"{"title":"Écoute", "browser":"Brave"}"#;
    const HOST: &str = "127.0.0.1:43210";
    const TOKEN: &str = "Bearer 0123456789abcdef0123456789abcdef";

    fn access() -> Access {
        Access {
            host: HOST.into(),
            authorization: TOKEN.into(),
        }
    }

    fn call(method: Method, path: &str) -> hyper::http::request::Builder {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, HOST)
            .header(header::ORIGIN, EXTENSION_ORIGIN)
            .header(header::AUTHORIZATION, TOKEN)
    }

    fn data(content: impl Into<Bytes>) -> Full<Bytes> {
        Full::new(content.into())
    }

    #[tokio::test]
    async fn only_the_paired_extension_can_submit_requests() {
        let registry = Mutex::new(Registry::default());
        let access = access();
        let request = Request::post("/v1/pairing")
            .header(header::HOST, HOST)
            .header(header::ORIGIN, "https://site.example")
            .header(header::AUTHORIZATION, TOKEN)
            .body(data(Bytes::new()))
            .unwrap();
        let denied = handle(&registry, &access, request).await;
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        assert!(
            !denied
                .headers()
                .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        );

        let request = Request::post("/v1/pairing")
            .header(header::HOST, HOST)
            .header(header::ORIGIN, EXTENSION_ORIGIN)
            .body(data(Bytes::new()))
            .unwrap();
        assert_eq!(
            handle(&registry, &access, request).await.status(),
            StatusCode::UNAUTHORIZED
        );

        let request = Request::builder()
            .method(Method::OPTIONS)
            .uri("/v1/pairing")
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
            .header(header::HOST, HOST)
            .header(header::ORIGIN, EXTENSION_ORIGIN)
            .body(data(Bytes::new()))
            .unwrap();
        let preflight = handle(&registry, &access, request).await;
        assert_eq!(preflight.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            preflight.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            EXTENSION_ORIGIN
        );

        let request = call(Method::POST, "/v1/pairing")
            .body(data(Bytes::new()))
            .unwrap();
        assert_eq!(
            handle(&registry, &access, request).await.status(),
            StatusCode::NO_CONTENT
        );
    }

    #[tokio::test]
    async fn browser_registration_routes_only_current_stereo_frames() {
        let capture = BrowserCapture::default();
        let registry = &capture.registry;
        let access = access();
        let start_path = format!("/v1/tabs/{TAB_ID}/start");
        let pcm_path = format!("/v1/tabs/{TAB_ID}/pcm");
        let stop_path = format!("/v1/tabs/{TAB_ID}");
        let start = || {
            call(Method::POST, &start_path)
                .header(header::CONTENT_TYPE, "application/json")
                .body(data(TITLE))
                .unwrap()
        };
        let pcm = || {
            call(Method::POST, &pcm_path)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(data(
                    [0.25_f32.to_le_bytes(), (-0.5_f32).to_le_bytes()].concat(),
                ))
                .unwrap()
        };
        let stop = || {
            call(Method::DELETE, &stop_path)
                .body(data(Bytes::new()))
                .unwrap()
        };

        assert_eq!(
            handle(registry, &access, start()).await.status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(capture.tabs()[0].title, "Écoute");
        // Audio submitted while the graph is stopped cannot reach a later reader.
        assert_eq!(
            handle(registry, &access, pcm()).await.status(),
            StatusCode::NO_CONTENT
        );
        let queue = capture.queue(TAB_ID);
        assert_eq!(queue.len(), 0);
        assert_eq!(
            handle(registry, &access, pcm()).await.status(),
            StatusCode::NO_CONTENT
        );
        let mut frame = [0.0_f32; 2];
        assert!(queue.pop(|channel, sample| frame[channel] = sample));
        assert_eq!(frame, [0.25, -0.5]);
        assert_eq!(
            handle(registry, &access, stop()).await.status(),
            StatusCode::NO_CONTENT
        );
        assert!(capture.tabs().is_empty());
        assert_eq!(
            handle(registry, &access, pcm()).await.status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            handle(registry, &access, start()).await.status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            handle(registry, &access, pcm()).await.status(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(queue.len(), 1);

        drop(queue);
        assert_eq!(capture.queue(TAB_ID).len(), 0);
        lock(registry).active.get_mut(TAB_ID).unwrap().last_seen = Instant::now() - TAB_IDLE;
        assert_eq!(
            handle(registry, &access, pcm()).await.status(),
            StatusCode::NOT_FOUND
        );
        assert!(capture.tabs().is_empty());
    }

    #[tokio::test]
    async fn pcm_requires_bounded_complete_stereo_frames() {
        let registry = Mutex::new(Registry::default());
        let access = access();
        let path = format!("/v1/tabs/{TAB_ID}/pcm");
        let upload = |size| {
            call(Method::POST, &path)
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(data(vec![0; size]))
                .unwrap()
        };
        assert_eq!(
            handle(&registry, &access, upload(7)).await.status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            handle(&registry, &access, upload(MAX_BODY + 8))
                .await
                .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            handle(&registry, &access, upload(8)).await.status(),
            StatusCode::NOT_FOUND
        );
    }
}
