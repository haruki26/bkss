use gpui::{App, AppContext, Context, Entity, WeakEntity};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::{Rc, Weak},
};

type Listener<T> = Rc<dyn Fn(&Publication<T>, &mut App) -> bool>;

pub(super) struct Publication<T> {
    pub(super) sequence: u64,
    pub(super) value: Rc<T>,
}

struct ListenerSet<T> {
    next_id: u64,
    listeners: Vec<(u64, Listener<T>)>,
}

/// A handle that unregisters a reactive observer when dropped.
pub struct ReactiveSubscription {
    cancel: Option<Box<dyn FnOnce()>>,
}

impl Drop for ReactiveSubscription {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel();
        }
    }
}

pub(super) struct ValueCell<T> {
    pub(super) value: Rc<T>,
    sequence: u64,
    listeners: Rc<RefCell<ListenerSet<T>>>,
    dispatching: bool,
    pending: VecDeque<Pending<T>>,
}

struct Pending<T> {
    value: T,
    alive: Option<Weak<Cell<bool>>>,
}

impl<T> Pending<T> {
    fn is_alive(&self) -> bool {
        self.alive
            .as_ref()
            .is_none_or(|alive| alive.upgrade().is_some_and(|alive| alive.get()))
    }
}

impl<T> ValueCell<T> {
    pub(super) fn new(value: T) -> Self {
        Self {
            value: Rc::new(value),
            sequence: 0,
            listeners: Rc::new(RefCell::new(ListenerSet {
                next_id: 0,
                listeners: Vec::new(),
            })),
            dispatching: false,
            pending: VecDeque::new(),
        }
    }
}

fn commit<T>(cell: &mut ValueCell<T>, value: T) -> (Publication<T>, Rc<RefCell<ListenerSet<T>>>) {
    cell.value = Rc::new(value);
    cell.sequence = cell.sequence.wrapping_add(1);
    (
        Publication {
            sequence: cell.sequence,
            value: Rc::clone(&cell.value),
        },
        Rc::clone(&cell.listeners),
    )
}

pub(super) type ObserverCallback<Owner, T> = Box<dyn FnMut(&mut Owner, &T, &mut Context<Owner>)>;

pub(super) struct Observer<Owner, T> {
    pub(super) callback: RefCell<Option<ObserverCallback<Owner, T>>>,
    pub(super) pending: RefCell<VecDeque<Rc<T>>>,
}

pub(super) fn invoke_observer<Owner: 'static, T: 'static>(
    observer: &Observer<Owner, T>,
    owner: &WeakEntity<Owner>,
    value: Rc<T>,
    app: &mut App,
) {
    let Some(owner_entity) = owner.upgrade() else {
        return;
    };
    let Some(mut callback) = observer.callback.borrow_mut().take() else {
        observer.pending.borrow_mut().push_back(value);
        return;
    };

    owner_entity.update(app, |owner, cx| callback(owner, value.as_ref(), cx));
    *observer.callback.borrow_mut() = Some(callback);

    loop {
        let value = observer.pending.borrow_mut().pop_front();
        let Some(value) = value else {
            break;
        };
        let Some(owner_entity) = owner.upgrade() else {
            break;
        };
        let Some(mut callback) = observer.callback.borrow_mut().take() else {
            observer.pending.borrow_mut().push_front(value);
            break;
        };
        owner_entity.update(app, |owner, cx| callback(owner, value.as_ref(), cx));
        *observer.callback.borrow_mut() = Some(callback);
    }
}

pub(super) fn listen<T: 'static, C: AppContext>(
    entity: &Entity<ValueCell<T>>,
    cx: &C,
    listener: impl Fn(&Publication<T>, &mut App) -> bool + 'static,
) -> ReactiveSubscription {
    let listeners = entity.read_with(cx, |cell, _| Rc::clone(&cell.listeners));
    let mut set = listeners.borrow_mut();
    let id = set.next_id;
    set.next_id = set.next_id.wrapping_add(1);
    set.listeners.push((id, Rc::new(listener)));
    drop(set);

    let listeners = Rc::downgrade(&listeners);
    ReactiveSubscription {
        cancel: Some(Box::new(move || {
            if let Some(listeners) = listeners.upgrade() {
                listeners
                    .borrow_mut()
                    .listeners
                    .retain(|(listener_id, _)| *listener_id != id);
            }
        })),
    }
}

fn dispatch<T>(registry: &Rc<RefCell<ListenerSet<T>>>, publication: Publication<T>, cx: &mut App) {
    let listeners = registry.borrow().listeners.clone();
    let mut remove = Vec::new();
    for (id, listener) in listeners {
        if !listener(&publication, cx) {
            remove.push(id);
        }
    }
    if !remove.is_empty() {
        registry
            .borrow_mut()
            .listeners
            .retain(|(id, _)| !remove.contains(id));
    }
}

pub(super) fn publish<T: 'static>(entity: &Entity<ValueCell<T>>, value: T, cx: &mut App) {
    publish_guarded(entity, value, None, cx);
}

pub(super) fn publish_guarded<T: 'static>(
    entity: &Entity<ValueCell<T>>,
    value: T,
    alive: Option<Weak<Cell<bool>>>,
    cx: &mut App,
) {
    let first = entity.update(cx, |cell, cx| {
        let pending = Pending { value, alive };
        if !pending.is_alive() {
            return None;
        }
        if cell.dispatching {
            cell.pending.push_back(pending);
            return None;
        }

        cell.dispatching = true;
        let publication = commit(cell, pending.value);
        cx.notify();
        Some(publication)
    });
    let Some(mut current) = first else {
        return;
    };

    loop {
        let (publication, listeners) = current;
        dispatch(&listeners, publication, cx);

        let next = entity.update(cx, |cell, cx| {
            loop {
                let Some(pending) = cell.pending.pop_front() else {
                    cell.dispatching = false;
                    break None;
                };
                if !pending.is_alive() {
                    continue;
                }
                let publication = commit(cell, pending.value);
                cx.notify();
                break Some(publication);
            }
        });
        let Some(next) = next else {
            break;
        };
        current = next;
    }
}

pub(super) fn publish_weak<T: 'static>(
    entity: &WeakEntity<ValueCell<T>>,
    value: T,
    cx: &mut App,
) -> bool {
    let Some(entity) = entity.upgrade() else {
        return false;
    };
    publish(&entity, value, cx);
    true
}
