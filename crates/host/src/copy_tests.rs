use super::*;

fn scripted_agent(mut copy: impl FnMut(u32, Request, &mut std::io::PipeWriter) -> bool + Send + 'static) -> RemoteHost {
    let (req_r, req_w) = std::io::pipe().unwrap();
    let (resp_r, resp_w) = std::io::pipe().unwrap();
    std::thread::spawn(move || {
        let mut r = BufReader::new(req_r);
        let mut w = resp_w;
        while let Ok(Some(Frame { id, body })) = proto::read_frame::<Request>(&mut r) {
            if let Request::Hello { client_version, .. } = body {
                proto::write_frame(
                    &mut w,
                    &Frame {
                        id,
                        body: Response::Hello(HostInfo {
                            version: client_version,
                            os: "test".into(),
                            arch: "test".into(),
                            home: "/".into(),
                            hostname: "test".into(),
                            resumed: false,
                        }),
                    },
                )
                .unwrap();
            } else if !copy(id, body, &mut w) {
                break;
            }
        }
    });
    RemoteHost::from_streams(Box::new(resp_r), Box::new(req_w), "copy-test").unwrap()
}

#[test]
fn copy_cancellation_waits_for_the_agents_cleanup_acknowledgement() {
    let (started_tx, started_rx) = mpsc::channel();
    let mut copy_id = None;
    let host = Arc::new(scripted_agent(move |id, request, writer| {
        match request {
            Request::Copy { .. } => {
                copy_id = Some(id);
                started_tx.send(()).unwrap();
            }
            Request::Cancel { id } => {
                assert_eq!(Some(id), copy_id);
                proto::write_frame(writer, &Frame { id, body: Response::Error(proto::Error::Cancelled) }).unwrap();
            }
            Request::Bye => return false,
            _ => {}
        }
        true
    }));
    let cancel = Arc::new(AtomicBool::new(false));
    let (worker_host, worker_cancel) = (host.clone(), cancel.clone());
    let worker = std::thread::spawn(move || worker_host.copy_path_cancellable("/source", "/target", &worker_cancel));
    started_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    cancel.store(true, Ordering::Release);
    assert!(matches!(worker.join().unwrap(), Err(HostError::Op(proto::Error::Cancelled))));
    assert!(host.shared.waiters.locked().is_empty());
}

#[test]
fn copy_connection_loss_has_an_explicit_uncertain_outcome() {
    let host = scripted_agent(|_, request, _| !matches!(request, Request::Copy { .. }));
    assert!(matches!(host.copy_path("/source", "/target"), Err(HostError::Uncertain(_))));
    assert!(host.shared.waiters.locked().is_empty());
}

#[test]
fn copy_cancelled_before_start_never_sends_a_mutating_request() {
    let mutated = Arc::new(AtomicBool::new(false));
    let observed = mutated.clone();
    let host = scripted_agent(move |_, request, _| {
        if matches!(request, Request::Copy { .. }) {
            observed.store(true, Ordering::Release);
        }
        !matches!(request, Request::Bye)
    });
    let cancel = AtomicBool::new(true);
    assert!(matches!(host.copy_path_cancellable("/source", "/target", &cancel), Err(HostError::Op(proto::Error::Cancelled))));
    assert!(!mutated.load(Ordering::Acquire));
}
