mod reply;
mod request;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

pub use reply::Reply;
pub use request::Request;

const HEAD_END: &[u8] = b"\r\n\r\n";

type Routes = Arc<BTreeMap<String, Reply>>;
type Recorded = Arc<Mutex<Vec<Request>>>;

pub struct Mock {
    base: String,
    recorded: Recorded,
    server: JoinHandle<()>,
}

impl Mock {
    pub async fn serve(routes: impl IntoIterator<Item = (&'static str, Reply)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding the mock server");
        let base = format!("http://{}", listener.local_addr().unwrap());
        let routes: Routes = Arc::new(
            routes
                .into_iter()
                .map(|(path, reply)| (path.to_owned(), reply))
                .collect(),
        );
        let recorded = Recorded::default();

        let server = tokio::spawn({
            let recorded = recorded.clone();
            async move {
                while let Ok((stream, _)) = listener.accept().await {
                    tokio::spawn(answer(stream, routes.clone(), recorded.clone()));
                }
            }
        });

        Self {
            base,
            recorded,
            server,
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn request(&self, path: &str) -> Request {
        self.recorded
            .lock()
            .unwrap()
            .iter()
            .find(|request| request.path == path)
            .cloned()
            .unwrap_or_else(|| panic!("nothing requested {path}"))
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn answer(mut stream: TcpStream, routes: Routes, recorded: Recorded) {
    let Some(request) = read_head(&mut stream).await else {
        return;
    };
    let reply = routes
        .get(&request.path)
        .cloned()
        .unwrap_or_else(|| Reply::status(404));
    recorded.lock().unwrap().push(request);

    let response = format!(
        "HTTP/1.1 {} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        reply.status,
        reply.body.len(),
        reply.body
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

async fn read_head(stream: &mut TcpStream) -> Option<Request> {
    let mut head = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !head
        .windows(HEAD_END.len())
        .any(|window| window == HEAD_END)
    {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        head.extend_from_slice(&chunk[..read]);
    }
    Request::parse(&String::from_utf8_lossy(&head))
}
