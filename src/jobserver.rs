use std::io;
use std::sync::Arc;
use tokio_02::process::Command;
use std::process::Command as StdCommand;

use futures_03::channel::mpsc;
use futures_03::channel::oneshot;
use futures_03::future;
use futures_03::prelude::*;

use crate::errors::*;

#[derive(Clone)]
pub struct Client {
    helper: Option<Arc<jobserver::HelperThread>>,
    tx: Option<mpsc::UnboundedSender<oneshot::Sender<io::Result<jobserver::Acquired>>>>,
    inner: jobserver::Client,
}

pub struct Acquired {
    _token: Option<jobserver::Acquired>,
}

impl Client {
    // unsafe because `from_env` is unsafe (can use the wrong fds)
    pub unsafe fn new() -> Client {
        match jobserver::Client::from_env() {
            Some(c) => Client::_new(c, true),
            None => Client::new_num(num_cpus::get()),
        }
    }

    pub fn new_num(num: usize) -> Client {
        let inner = jobserver::Client::new(num).expect("failed to create jobserver");
        Client::_new(inner, false)
    }

    fn _new(inner: jobserver::Client, inherited: bool) -> Client {
        let (helper, tx) = if inherited {
            (None, None)
        } else {
            let (tx, rx) = mpsc::unbounded::<oneshot::Sender<_>>();
            let helper = inner
                .clone()
                .into_helper_thread(move |token| {
                    tokio_02::runtime::Runtime::new()
                        .unwrap()
                        .block_on(async move {
                            if let Some(sender) = rx.next().await {
                                drop(sender.send(token));
                            }
                        });
                })
                .expect("failed to spawn helper thread");
            (Some(Arc::new(helper)), Some(tx))
        };

        Client { inner, helper, tx }
    }

    /// Configures this jobserver to be inherited by the specified command
    pub fn configure(&self, cmd: &mut StdCommand) {
        self.inner.configure(cmd)
    }

    /// Returns a future that represents an acquired jobserver token.
    ///
    /// This should be invoked before any "work" is spawned (for whatever the
    /// definition of "work" is) to ensure that the system is properly
    /// rate-limiting itself.
    pub async fn acquire(&self) -> Result<Acquired> {
        let (helper, tx) = match (self.helper.as_ref(), self.tx.as_ref()) {
            (Some(a), Some(b)) => (a, b),
            _ => return Ok(Acquired { _token: None }),
        };
        let (mytx, myrx) = oneshot::channel();
        helper.request_token();
        tx.unbounded_send(mytx).unwrap();

        let acquired = myrx
            .await
            .context("jobserver helper panicked")?
            .context("failed to acquire jobserver token")?;
        Ok(Acquired {
            _token: Some(acquired),
        })
    }
}
