use std::future::Future;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::sync::Mutex;

use super::reply::{HttpReply, LiveLimits, Outgoing, Session};
use super::write_timeout::WriteTimeout;
use super::{Budget, Limits, Request, RequestContext, read_request, reject};
use crate::{net, task};

/// HTTP/1 每条连接最多一个活跃 handler；升级后仍占用同一个连接和任务名额。
pub async fn serve_live<F, Fut>(
    route: F,
    listener: net::Listener,
    limits: Limits,
    live: LiveLimits,
) -> Result<(), String>
where
    F: Fn(Request, HttpReply) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    serve_transport(route, listener, limits, live, None).await
}

pub async fn serve_live_tls<F, Fut>(
    route: F,
    listener: net::Listener,
    limits: Limits,
    live: LiveLimits,
    tls: crate::tls::ServerTls,
) -> Result<(), String>
where
    F: Fn(Request, HttpReply) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    serve_transport(route, listener, limits, live, Some(tls)).await
}

async fn serve_transport<F, Fut>(
    route: F,
    listener: net::Listener,
    limits: Limits,
    live: LiveLimits,
    tls: Option<crate::tls::ServerTls>,
) -> Result<(), String>
where
    F: Fn(Request, HttpReply) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    let budget = limits.check()?;
    live.check()?;
    let tls = tls.map(|tls| super::protocol::server_tls(tls, budget.http2));
    task::parallel_each_stream(
        net::incoming(listener.clone(), true),
        limits.connections,
        move |connection| {
            let route = route.clone();
            let tls = tls.clone();
            let listener = listener.clone();
            async move {
                let stream = connection?;
                let stream = match crate::tls::accept(stream, tls.as_ref(), limits.timeout_ms).await
                {
                    Ok(stream) => stream,
                    Err(_) => return Ok(()),
                };
                if super::protocol::check_alpn(&stream, budget.http2).is_err() {
                    return Ok(());
                }
                if let Some(http2) = budget.http2 {
                    return serve_http2(route, listener, stream, budget, live, http2).await;
                }
                let handlers = Arc::new(Mutex::new(Some(task::Group::new(1)?)));
                let upgraded = Arc::new(AtomicBool::new(false));
                let service = {
                    let handlers = handlers.clone();
                    let upgraded = upgraded.clone();
                    service_fn(move |request| {
                        dispatch(
                            request,
                            route.clone(),
                            budget,
                            live,
                            handlers.clone(),
                            upgraded.clone(),
                        )
                    })
                };
                let connection = super::server_builder(budget)
                    .serve_connection(
                        TokioIo::new(WriteTimeout::new(stream, budget.timeout)),
                        service,
                    )
                    .with_upgrades();
                tokio::pin!(connection);
                let result = tokio::select! {
                    result = &mut connection => result,
                    _ = listener.closed() => {
                        connection.as_mut().graceful_shutdown();
                        connection.await
                    }
                };
                let group = handlers
                    .lock()
                    .await
                    .take()
                    .expect("connection owns handlers");
                if result.is_ok() && upgraded.load(Ordering::Acquire) {
                    group.wait().await?;
                } else {
                    group.stop().await?;
                }
                super::connection_result(result)
            }
        },
    )
    .await
}

async fn serve_http2<F, Fut>(
    route: F,
    listener: net::Listener,
    stream: crate::transport::Transport,
    budget: Budget,
    live: LiveLimits,
    http2: super::Http2Limits,
) -> Result<(), String>
where
    F: Fn(Request, HttpReply) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    let (mut owner, spawner, fault) = task::scoped_owner(())?;
    let first_request = super::server_h2::FirstRequest::new();
    let service = {
        let first_request = first_request.clone();
        service_fn(move |request| {
            first_request.mark();
            let route = route.clone();
            dispatch_http2(request, route, budget, live)
        })
    };
    let mut builder = http2.server_builder(super::executor::Http2Executor(spawner), budget.headers);
    super::server_h2::configure_liveness(&mut builder, budget.timeout);
    let connection = builder.serve_connection(
        hyper_util::rt::TokioIo::new(WriteTimeout::new(stream, budget.timeout)),
        service,
    );
    let result = owner
        .enter(super::server_h2::drive(
            connection,
            &listener,
            &first_request,
            budget.timeout,
            &fault,
            |connection| connection.graceful_shutdown(),
        ))
        .await;
    super::server_h2::complete(owner, result).await
}

async fn dispatch<F, Fut>(
    mut request: hyper::Request<hyper::body::Incoming>,
    route: F,
    budget: Budget,
    live: LiveLimits,
    handlers: Arc<Mutex<Option<task::Group>>>,
    upgraded: Arc<AtomicBool>,
) -> Result<hyper::Response<Outgoing>, std::convert::Infallible>
where
    F: Fn(Request, HttpReply) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    let upgrade = if request.headers().contains_key(hyper::header::UPGRADE) {
        let upgrade = hyper::upgrade::on(&mut request);
        let mut handshake = hyper::Request::new(());
        *handshake.method_mut() = request.method().clone();
        *handshake.uri_mut() = request.uri().clone();
        *handshake.version_mut() = request.version();
        *handshake.headers_mut() = request.headers().clone();
        Some((handshake, upgrade))
    } else {
        None
    };
    let request = match read_request(request, budget).await {
        Ok(request) => request,
        Err(status) => return Ok(reject(status, budget).map(Outgoing::Full)),
    };
    let context = RequestContext::new("http/1.1", request.target.clone(), None);
    let (session, head) = Session::new(budget, live, upgrade, upgraded);
    let reply = session.reply();
    let state = reply.clone();
    let handler_context = context.clone();
    let started = handlers
        .lock()
        .await
        .as_mut()
        .expect("active connection")
        .run(async move {
            let result = session.run(route(request, reply)).await;
            if let Err(error) = result {
                handler_context.report(error);
            }
            Ok(())
        })
        .await;
    if let Err(error) = started {
        context.report(error);
        return Ok(reject(hyper::StatusCode::INTERNAL_SERVER_ERROR, budget).map(Outgoing::Full));
    }
    match tokio::time::timeout(budget.timeout, head).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(_)) => {
            if state.fault().is_none() {
                context.report("HTTP live handler ended without a response");
            }
            Ok(reject(hyper::StatusCode::INTERNAL_SERVER_ERROR, budget).map(Outgoing::Full))
        }
        Err(_) => Ok(reject(hyper::StatusCode::GATEWAY_TIMEOUT, budget).map(Outgoing::Full)),
    }
}

async fn dispatch_http2<F, Fut>(
    request: hyper::Request<hyper::body::Incoming>,
    route: F,
    budget: Budget,
    live: LiveLimits,
) -> Result<hyper::Response<Outgoing>, std::convert::Infallible>
where
    F: Fn(Request, HttpReply) -> Fut + Send + 'static,
    Fut: Future<Output = Result<(), String>> + Send + 'static,
{
    if request.method() == hyper::Method::CONNECT
        || request.headers().contains_key(hyper::header::UPGRADE)
    {
        return Ok(reject(hyper::StatusCode::NOT_IMPLEMENTED, budget).map(Outgoing::Full));
    }
    let request = match read_request(request, budget).await {
        Ok(request) => request,
        Err(status) => return Ok(reject(status, budget).map(Outgoing::Full)),
    };
    let context = RequestContext::new("http/2", request.target.clone(), None);
    let (session, head) = Session::new(budget, live, None, Arc::new(AtomicBool::new(false)));
    let reply = session.reply();
    let state = reply.clone();
    let mut handler = match task::Group::new(1) {
        Ok(handler) => handler,
        Err(error) => {
            context.report(error);
            return Ok(reject(hyper::StatusCode::INTERNAL_SERVER_ERROR, budget).map(Outgoing::Full));
        }
    };
    let handler_context = context.clone();
    let started = handler
        .run(async move {
            let result = session.run(route(request, reply)).await;
            if let Err(error) = result {
                handler_context.report(error);
            }
            Ok(())
        })
        .await;
    if let Err(error) = started {
        context.report(error);
        return Ok(reject(hyper::StatusCode::INTERNAL_SERVER_ERROR, budget).map(Outgoing::Full));
    }
    match tokio::time::timeout(budget.timeout, head).await {
        Ok(Ok(response)) => Ok(response.map(|body| body.with_handler(handler))),
        Ok(Err(_)) => {
            if let Err(error) = handler.wait().await {
                context.report(error);
            } else if state.fault().is_none() {
                context.report("HTTP live handler ended without a response");
            }
            Ok(reject(hyper::StatusCode::INTERNAL_SERVER_ERROR, budget).map(Outgoing::Full))
        }
        Err(_) => match handler.stop().await {
            Ok(()) => Ok(reject(hyper::StatusCode::GATEWAY_TIMEOUT, budget).map(Outgoing::Full)),
            Err(error) => {
                context.report(error);
                Ok(reject(hyper::StatusCode::GATEWAY_TIMEOUT, budget).map(Outgoing::Full))
            }
        },
    }
}
