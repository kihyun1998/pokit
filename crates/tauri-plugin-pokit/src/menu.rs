//! Watching the menus a right-click pokit delivered opens, and closing one when pokit asks.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{NSMenu, NSMenuDidBeginTrackingNotification, NSMenuDidEndTrackingNotification};
use objc2_foundation::{
    NSNotification, NSNotificationCenter, NSNotificationName, NSOperationQueue,
};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::ptr::NonNull;
use std::sync::{Condvar, Mutex, MutexGuard, Once};
use std::time::{Duration, Instant};

/// What the watch has seen, by right-click number.
struct Watch {
    /// How many right-clicks have been delivered.
    clicks: u64,
    /// The right-click current when the open menu began tracking; 0 when no menu is open.
    open_since: u64,
    /// The latest right-click whose menu was closed.
    closed: u64,
    /// The latest right-click whose menu had already gone when pokit came to close it.
    gone: u64,
}

static WATCH: Mutex<Watch> = Mutex::new(Watch {
    clicks: 0,
    open_since: 0,
    closed: 0,
    gone: 0,
});
static CHANGED: Condvar = Condvar::new();
static OBSERVE: Once = Once::new();

thread_local! {
    /// The menu tracking now, on the main thread.
    static OPEN: RefCell<Option<Retained<NSMenu>>> = const { RefCell::new(None) };
}

/// Counts a right-click about to be delivered, and returns its number. Runs on the main thread,
/// where menus post their notifications.
pub fn arm() -> u64 {
    OBSERVE.call_once(|| {
        // SAFETY: AppKit's notification names are immutable statics.
        let (began, ended) = unsafe {
            (
                NSMenuDidBeginTrackingNotification,
                NSMenuDidEndTrackingNotification,
            )
        };
        observe(began, |menu| {
            OPEN.with(|open| *open.borrow_mut() = Some(menu));
            let mut w = WATCH.lock().unwrap();
            w.open_since = w.clicks;
            CHANGED.notify_all();
        });
        observe(ended, |_| {
            OPEN.with(|open| *open.borrow_mut() = None);
            WATCH.lock().unwrap().open_since = 0;
        });
    });
    let mut w = WATCH.lock().unwrap();
    w.clicks += 1;
    w.clicks
}

/// Runs `then` on the main thread with the menu each `name` notification is about.
fn observe(name: &'static NSNotificationName, then: impl Fn(Retained<NSMenu>) + 'static) {
    let block = RcBlock::new(move |note: NonNull<NSNotification>| {
        // SAFETY: the notification centre hands the block a live notification.
        let note = unsafe { note.as_ref() };
        if let Some(menu) = note.object().and_then(|o| o.downcast::<NSMenu>().ok()) {
            then(menu);
        }
    });
    // SAFETY: a nil queue runs the block on the posting thread, AppKit's main thread.
    let observer = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(name),
            None,
            None,
            &block,
        )
    };
    std::mem::forget(observer);
}

/// Waits up to `wait` for a menu to open after right-click `click`, closes it from inside its
/// tracking loop, and says whether it did.
pub fn close(click: u64, wait: Duration) -> Value {
    let until = Instant::now() + wait;
    let mut w = wait_while(WATCH.lock().unwrap(), until, |w| {
        w.open_since < click && w.closed < click
    });
    if w.open_since >= click && w.closed < click {
        let cancel = RcBlock::new(move || {
            let menu = OPEN.with(|open| open.borrow_mut().take());
            if let Some(menu) = &menu {
                menu.cancelTrackingWithoutAnimation();
            }
            let mut w = WATCH.lock().unwrap();
            match menu {
                Some(_) => w.closed = w.closed.max(click),
                None => w.gone = w.gone.max(click),
            }
            CHANGED.notify_all();
        });
        // SAFETY: the main queue runs the block on the main thread, where the menu lives.
        unsafe { NSOperationQueue::mainQueue().addOperationWithBlock(&cancel) };
        w = wait_while(w, until, |w| w.closed < click && w.gone < click);
    }
    json!({ "ok": true, "closed": w.closed >= click })
}

/// Waits on the watch while `pending` holds, until `until` at the latest.
fn wait_while(
    mut w: MutexGuard<'static, Watch>,
    until: Instant,
    pending: impl Fn(&Watch) -> bool,
) -> MutexGuard<'static, Watch> {
    while pending(&w) {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        w = CHANGED.wait_timeout(w, left).unwrap().0;
    }
    w
}
