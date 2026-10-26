//! browser suiteの子process回収と実行期限を共有する。診断にraw IPCを複製しない。
use super::*;
use std::process::{Child, ExitStatus};
use std::sync::mpsc;

pub(super) struct BrowserProcess(pub(super) Option<Child>);
impl BrowserProcess {
    pub(super) fn child(&mut self) -> &mut Child {
        self.0.as_mut().expect("browser process retained")
    }

    pub(super) fn wait_bounded(mut self, timeout: Duration) -> Result<ExitStatus> {
        let mut child = self.0.take().expect("browser process retained");
        let pid = child.id() as libc::pid_t;
        let (send, receive) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let _ = send.send(child.wait());
        });
        let result = receive.recv_timeout(timeout);
        if result.is_err() {
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
            if receive.recv_timeout(Duration::from_secs(3)).is_err() {
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
        let _ = waiter.join();
        Ok(result.map_err(|_| "browser E2E timed out; see sanitized browser diagnostic")??)
    }
}
impl Drop for BrowserProcess {
    fn drop(&mut self) {
        let Some(mut child) = self.0.take() else {
            return;
        };
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        let pid = child.id() as libc::pid_t;
        // Denoのsignal handlerがPlaywright/HTTPを閉じる猶予を与える。
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
        let (send, receive) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let _ = send.send(child.wait());
        });
        if receive.recv_timeout(Duration::from_secs(3)).is_err() {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
        let _ = waiter.join();
    }
}
