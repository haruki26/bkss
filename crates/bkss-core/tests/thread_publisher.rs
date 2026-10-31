use bkss_core::{
    ResourceScope, Source, SourceDescriptor, SourcePublisher, SourceState, ThreadPublishError,
    ThreadPublisher, ThreadPublisherGuard,
};
use gpui::{App, AppContext, TestAppContext};
use std::{cell::RefCell, error::Error, fmt, rc::Rc, thread};

struct LocalError(Rc<String>);

impl fmt::Display for LocalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for LocalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Error for LocalError {}

struct ThreadSource {
    senders: Rc<RefCell<Vec<ThreadPublisher<i32, String>>>>,
    map_value_local: Rc<String>,
}

struct BridgeGuard {
    _bridge: ThreadPublisherGuard,
}

impl Source for ThreadSource {
    type Key = ();
    type Value = Rc<Vec<i32>>;
    type Error = LocalError;
    type Guard = BridgeGuard;

    fn key(&self) {}

    fn start(
        &self,
        publisher: SourcePublisher<Self>,
        cx: &mut App,
    ) -> Result<Self::Guard, Self::Error> {
        let local = Rc::clone(&self.map_value_local);
        let error_prefix = Rc::clone(&self.map_value_local);
        let (sender, guard) = publisher.thread_bridge(
            move |value| {
                assert_eq!(local.as_str(), "UI-local mapping");
                Rc::new(vec![value])
            },
            move |message| LocalError(Rc::new(format!("{error_prefix}: {message}"))),
            cx,
        );
        self.senders.borrow_mut().push(sender);
        Ok(BridgeGuard { _bridge: guard })
    }
}

#[test]
fn thread_bridge_composes_with_source_lifecycle_and_preserves_payload_cause() {
    let cx = TestAppContext::single();
    let senders = Rc::new(RefCell::new(Vec::new()));
    let descriptor = SourceDescriptor::new(ThreadSource {
        senders: Rc::clone(&senders),
        map_value_local: Rc::new("UI-local mapping".to_owned()),
    });
    let scope = cx.update(ResourceScope::new);
    let lease = cx.update(|cx| scope.acquire(&descriptor, cx).unwrap());
    assert_eq!(lease.reactive().get(&cx), SourceState::Loading);

    let observed_values = Rc::new(RefCell::new(Vec::new()));
    let output = Rc::clone(&observed_values);
    let observed = lease.reactive();
    let owner = cx.update(|cx| cx.new(|_| ()));
    let _subscription = cx.update(|cx| {
        owner.update(cx, |_, owner_cx| {
            observed.observe(owner_cx, move |_, state, _| {
                if let SourceState::Ready(value) = state {
                    output.borrow_mut().push(value.clone());
                }
            })
        })
    });

    let original_sender = senders.borrow()[0].clone();
    let worker = original_sender.clone();
    thread::spawn(move || {
        worker.publish(7).unwrap();
        worker.publish(7).unwrap();
    })
    .join()
    .unwrap();
    assert_eq!(lease.reactive().get(&cx), SourceState::Loading);
    cx.run_until_parked();
    assert_eq!(
        lease.reactive().get(&cx),
        SourceState::Ready(Rc::new(vec![7]))
    );
    assert_eq!(
        *observed_values.borrow(),
        [Rc::new(vec![7]), Rc::new(vec![7])],
        "both equal explicit publications are notified"
    );

    drop(lease);
    assert_eq!(
        original_sender.publish(9),
        Err(ThreadPublishError::Closed(9)),
        "the Source guard owns and closes the bridge"
    );

    let next = cx.update(|cx| scope.acquire(&descriptor, cx).unwrap());
    assert_eq!(next.reactive().get(&cx), SourceState::Loading);
    assert_eq!(senders.borrow().len(), 2);
    drop(next);

    let failure_senders = Rc::new(RefCell::new(Vec::new()));
    let failed_descriptor = SourceDescriptor::new(ThreadSource {
        senders: Rc::clone(&failure_senders),
        map_value_local: Rc::new("UI-local mapping".to_owned()),
    });
    let failed_scope = cx.update(ResourceScope::new);
    let _failed_lease = cx.update(|cx| failed_scope.acquire(&failed_descriptor, cx).unwrap());
    let failure_sender = failure_senders.borrow()[0].clone();
    let worker = failure_sender.clone();
    thread::spawn(move || worker.fail("typed failure cause".to_owned()).unwrap())
        .join()
        .unwrap();
    cx.run_until_parked();
    let failure = failed_scope.failure().expect("failure reaches the scope");
    assert!(
        failure
            .source()
            .and_then(|cause| cause.downcast_ref::<LocalError>())
            .is_some_and(|error| error.0.as_str() == "UI-local mapping: typed failure cause")
    );
    assert_eq!(
        failure_sender.fail("after shutdown".to_owned()),
        Err(ThreadPublishError::Closed("after shutdown".to_owned()))
    );
}
