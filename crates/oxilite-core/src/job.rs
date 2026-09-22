//! Step machines: the sans-IO execution model.
//!
//! A [`Job`] is driven by repeatedly calling [`Job::step`] with the response to the previous
//! request. Drivers are trivial loops, see [`run_sync`].
//!
// @lat: [[architecture#Sans-IO core]]

use crate::error::Result;
use crate::sql::{Request, Response};

/// What a job wants next.
#[derive(Debug)]
pub enum Step<T> {
    /// Run this request and resume with its response.
    Execute(Request),
    /// Finished.
    Done(T),
}

/// A resumable operation.
pub trait Job {
    type Output;

    /// Advances the job. The first call gets `None`; later calls get the response to the
    /// previously returned request.
    fn step(&mut self, response: Option<Response>) -> Result<Step<Self::Output>>;
}

/// A backend that runs requests synchronously.
pub trait SyncBackend {
    fn execute(&self, request: &Request) -> Result<Response>;
    fn capabilities(&self) -> &crate::sql::Capabilities;

    /// Starts an interactive transaction (savepoint). Only for backends with
    /// `interactive_transactions`.
    fn begin(&self) -> Result<()> {
        Err(crate::Error::unsupported("interactive transactions"))
    }
    fn commit(&self) -> Result<()> {
        Err(crate::Error::unsupported("interactive transactions"))
    }
    fn rollback(&self) -> Result<()> {
        Err(crate::Error::unsupported("interactive transactions"))
    }
}

impl<B: SyncBackend + ?Sized> SyncBackend for &B {
    fn execute(&self, request: &Request) -> Result<Response> {
        (**self).execute(request)
    }
    fn capabilities(&self) -> &crate::sql::Capabilities {
        (**self).capabilities()
    }
    fn begin(&self) -> Result<()> {
        (**self).begin()
    }
    fn commit(&self) -> Result<()> {
        (**self).commit()
    }
    fn rollback(&self) -> Result<()> {
        (**self).rollback()
    }
}

/// A backend that runs requests asynchronously (D1, network engines).
///
/// Futures are not required to be `Send`: Workers are single-threaded.
#[allow(async_fn_in_trait)]
pub trait AsyncBackend {
    async fn execute(&self, request: &Request) -> Result<Response>;
    fn capabilities(&self) -> &crate::sql::Capabilities;
}

/// Runs a job to completion on a sync backend.
pub fn run_sync<J: Job>(backend: &impl SyncBackend, mut job: J) -> Result<J::Output> {
    let mut response = None;
    loop {
        match job.step(response.take())? {
            Step::Execute(request) => response = Some(backend.execute(&request)?),
            Step::Done(out) => return Ok(out),
        }
    }
}

/// Runs a job to completion on an async backend.
pub async fn run_async<J: Job>(backend: &impl AsyncBackend, mut job: J) -> Result<J::Output> {
    let mut response = None;
    loop {
        match job.step(response.take())? {
            Step::Execute(request) => response = Some(backend.execute(&request).await?),
            Step::Done(out) => return Ok(out),
        }
    }
}

/// A job made of one request and a decoder.
pub struct OneShot<T> {
    request: Option<Request>,
    decode: Option<Box<dyn FnOnce(Response) -> Result<T>>>,
}

impl<T> OneShot<T> {
    pub fn new(request: Request, decode: impl FnOnce(Response) -> Result<T> + 'static) -> Self {
        Self {
            request: Some(request),
            decode: Some(Box::new(decode)),
        }
    }
}

impl<T> Job for OneShot<T> {
    type Output = T;

    fn step(&mut self, response: Option<Response>) -> Result<Step<T>> {
        match response {
            None => Ok(Step::Execute(
                self.request.take().expect("OneShot started twice"),
            )),
            Some(r) => Ok(Step::Done((self
                .decode
                .take()
                .expect("OneShot resumed twice"))(
                r
            )?)),
        }
    }
}

/// A job running a sequence of requests (e.g. chunked bulk loads), returning total changes
/// of the statements flagged in `count`.
pub struct Sequence {
    requests: std::vec::IntoIter<Request>,
    changes: u64,
}

impl Sequence {
    pub fn new(requests: Vec<Request>) -> Self {
        Self {
            requests: requests.into_iter(),
            changes: 0,
        }
    }
}

impl Job for Sequence {
    type Output = u64;

    fn step(&mut self, response: Option<Response>) -> Result<Step<u64>> {
        if let Some(r) = response {
            self.changes += r.iter().map(|rs| rs.changes).sum::<u64>();
        }
        Ok(match self.requests.next() {
            Some(r) => Step::Execute(r),
            None => Step::Done(self.changes),
        })
    }
}
