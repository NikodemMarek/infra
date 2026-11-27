#![allow(dead_code)]

use std::io::Read;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind a free port")
        .local_addr()
        .expect("local addr")
        .port()
}

pub fn closed_url() -> String {
    format!("nats://127.0.0.1:{}", free_port())
}

pub struct NatsServer {
    port: u16,
    jetstream: bool,
    dir: PathBuf,
    child: Option<Child>,
}

impl NatsServer {
    pub fn start() -> NatsServer {
        NatsServer::spawn(true)
    }

    pub fn start_without_jetstream() -> NatsServer {
        NatsServer::spawn(false)
    }

    fn spawn(jetstream: bool) -> NatsServer {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "jw-nats-{}-{}-{nanos}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("create the nats store dir");
        let mut server = NatsServer {
            port: 0,
            jetstream,
            dir,
            child: None,
        };
        server.launch();
        server
    }

    fn launch(&mut self) {
        let first = self.port == 0;
        let port = if first { -1 } else { i32::from(self.port) };
        let mut cmd = Command::new("nats-server");
        cmd.args(["-a", "127.0.0.1", "-p", &port.to_string()]);
        if self.jetstream {
            cmd.arg("-js");
        }
        cmd.arg("-sd")
            .arg(self.dir.join("store"))
            .arg("--ports_file_dir")
            .arg(&self.dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                panic!("nats-server not on PATH")
            }
            Err(e) => panic!("cannot run nats-server: {e}"),
        };
        let pid = child.id();
        self.child = Some(child);
        let deadline = Instant::now() + Duration::from_secs(20);
        if first {
            self.port = self.read_port(pid, deadline);
        }
        self.wait_for_info(deadline);
    }

    fn check_alive(&mut self) {
        if let Some(status) = self.child.as_mut().unwrap().try_wait().unwrap() {
            panic!("nats-server exited early: {status}");
        }
    }

    fn read_port(&mut self, pid: u32, deadline: Instant) -> u16 {
        let file = self.dir.join(format!("nats-server_{pid}.ports"));
        let prefix = "nats://127.0.0.1:";
        loop {
            if let Ok(text) = std::fs::read_to_string(&file) {
                if let Some(i) = text.find(prefix) {
                    let digits: String = text[i + prefix.len()..]
                        .chars()
                        .take_while(char::is_ascii_digit)
                        .collect();
                    if let Ok(port) = digits.parse() {
                        return port;
                    }
                }
            }
            self.check_alive();
            if Instant::now() > deadline {
                panic!(
                    "nats-server did not report its port within 20 s ({})",
                    file.display()
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn wait_for_info(&mut self, deadline: Instant) {
        let addr: SocketAddr = format!("127.0.0.1:{}", self.port).parse().unwrap();
        loop {
            if let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(500)) {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                let mut buf = [0u8; 4];
                if stream.read_exact(&mut buf).is_ok() && &buf == b"INFO" {
                    return;
                }
            }
            self.check_alive();
            if Instant::now() > deadline {
                panic!(
                    "nats-server on port {} did not answer with INFO within 20 s",
                    self.port
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    pub fn url(&self) -> String {
        format!("nats://127.0.0.1:{}", self.port)
    }

    pub fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    pub fn restart(&mut self) {
        self.stop();
        self.launch();
    }
}

impl Drop for NatsServer {
    fn drop(&mut self) {
        self.stop();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
