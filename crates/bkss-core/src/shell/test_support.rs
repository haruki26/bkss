use crate::{Source, SourceDescriptor, SourcePublisher};
use gpui::App;
use std::{
    cell::{Cell, RefCell},
    error::Error,
    fmt,
    rc::Rc,
};

#[derive(Debug)]
pub(super) struct TestError(pub(super) &'static str);

impl fmt::Display for TestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Error for TestError {}

pub(super) struct TestSource {
    pub(super) key: u8,
    pub(super) starts: Rc<Cell<usize>>,
    pub(super) stops: Rc<Cell<usize>>,
    pub(super) publisher: Rc<RefCell<Option<SourcePublisher<Self>>>>,
    pub(super) fails: bool,
    pub(super) on_start: Option<TestStart>,
    pub(super) on_drop: Option<Rc<dyn Fn()>>,
}

pub(super) struct TestGuard(pub(super) Rc<Cell<usize>>, Option<Rc<dyn Fn()>>);

pub(super) type TestDescriptor = SourceDescriptor<TestSource>;
pub(super) type TestPublisher = Rc<RefCell<Option<SourcePublisher<TestSource>>>>;
pub(super) type TestStart = Rc<dyn Fn(SourcePublisher<TestSource>, &mut App)>;

impl Drop for TestGuard {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
        if let Some(on_drop) = &self.1 {
            on_drop();
        }
    }
}

impl Source for TestSource {
    type Key = u8;
    type Value = u8;
    type Error = TestError;
    type Guard = TestGuard;

    fn key(&self) -> u8 {
        self.key
    }

    fn start(
        &self,
        publisher: SourcePublisher<Self>,
        cx: &mut App,
    ) -> Result<Self::Guard, Self::Error> {
        self.starts.set(self.starts.get() + 1);
        *self.publisher.borrow_mut() = Some(publisher.clone());
        if let Some(on_start) = &self.on_start {
            on_start(publisher, cx);
        }
        if self.fails {
            Err(TestError("source start"))
        } else {
            Ok(TestGuard(Rc::clone(&self.stops), self.on_drop.clone()))
        }
    }
}

pub(super) fn descriptor(
    starts: Rc<Cell<usize>>,
    stops: Rc<Cell<usize>>,
    fails: bool,
) -> (TestDescriptor, TestPublisher) {
    descriptor_with_options(starts, stops, fails, None, None, 1)
}

pub(super) fn descriptor_with_options(
    starts: Rc<Cell<usize>>,
    stops: Rc<Cell<usize>>,
    fails: bool,
    on_start: Option<TestStart>,
    on_drop: Option<Rc<dyn Fn()>>,
    key: u8,
) -> (TestDescriptor, TestPublisher) {
    let publisher = Rc::new(RefCell::new(None));
    (
        SourceDescriptor::new(TestSource {
            key,
            starts,
            stops,
            publisher: Rc::clone(&publisher),
            fails,
            on_start,
            on_drop,
        }),
        publisher,
    )
}
