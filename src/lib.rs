use futures::channel::oneshot;
use std::borrow::Cow;
use std::future::Future;

pub use puppet_derive::puppet_actor;

// Not public API. Used by generated code.
#[doc(hidden)]
pub mod __private {
    pub use flume;
    pub use futures;

    #[cfg(feature = "helper-methods")]
    pub use tokio;
}

/// A derived actor handler.
pub trait Actor {
    /// The derive actor messages.
    type Messages;
}

/// An actor message.
pub trait Message {
    /// The response type of the message.
    type Output;
}

/// A custom executor to spawn actors into.
pub trait Executor {
    fn spawn(&self, fut: impl Future<Output = ()> + Send + 'static);
}

// Not public API. Used by generated code.
#[doc(hidden)]
pub trait MessageHandler<T: Message> {
    fn create(msg: T) -> (Self, oneshot::Receiver<T::Output>)
    where
        Self: Sized;
}

/// A actor mailbox.
///
/// This is a cheap to clone way of contacting the actor and sending messages.
pub struct ActorMailbox<A: Actor> {
    tx: flume::Sender<A::Messages>,
    name: Cow<'static, str>,
}

impl<A: Actor> Clone for ActorMailbox<A> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            name: self.name.clone(),
        }
    }
}

impl<A: Actor> ActorMailbox<A> {
    // Not public API. Used by generated code.
    #[doc(hidden)]
    /// Creates a new actor mailbox.
    ///
    /// This should only really be made by the derive system.
    pub fn new(tx: flume::Sender<A::Messages>, name: Cow<'static, str>) -> Self {
        Self { tx, name }
    }

    #[inline]
    /// The name of the actor.
    ///
    /// This can be set by calling `actor.spawn_actor_with_name` or `actor.spawn_actor_with_name_and_size`
    pub fn name(&self) -> &str {
        self.name.as_ref()
    }

    /// Sends a message to the actor and waits for a response back.
    pub async fn send<T>(&self, msg: T) -> T::Output
    where
        T: Message,
        A::Messages: MessageHandler<T>,
    {
        let (msg, rx) = A::Messages::create(msg);
        self.tx.send_async(msg).await.expect("Contact actor");
        rx.await.expect("Actor response")
    }

    /// Sends a message to the actor and returns a deferred response.
    ///
    /// This does not wait for the returned message.
    pub async fn deferred_send<T>(&self, msg: T) -> DeferredResponse<T::Output>
    where
        T: Message,
        A::Messages: MessageHandler<T>,
    {
        let (msg, rx) = A::Messages::create(msg);
        self.tx.send_async(msg).await.expect("Contact actor");
        DeferredResponse { rx }
    }

    /// Sends a message to the actor and waits for a response back.
    ///
    /// This a sync variant which will block the thread until the message is returned.
    pub fn send_sync<T>(&self, msg: T) -> T::Output
    where
        T: Message,
        A::Messages: MessageHandler<T>,
    {
        let (msg, rx) = A::Messages::create(msg);
        self.tx.send(msg).expect("Contact actor");
        futures::executor::block_on(rx).expect("Actor response")
    }
}

/// A deferred response from the actor.
///
/// This can be used to schedule a message while not waiting for the result.
pub struct DeferredResponse<T> {
    rx: oneshot::Receiver<T>,
}

impl<T> DeferredResponse<T> {
    /// Attempts to get the result of the response immediately.
    pub fn try_recv(&mut self) -> Option<T> {
        self.rx.try_recv().expect("Get actor response")
    }

    /// Waits for the response by the actor.
    pub async fn recv(self) -> T {
        self.rx.await.expect("Get actor response")
    }
}

/// A reply handle.
///
/// Because it cannot be guaranteed that the reply will be called,
/// we require that response `T` implements default to act as a safe reply
/// method if `reply()` is not called directly.
pub struct Reply<T: Default> {
    tx: Option<oneshot::Sender<T>>,
}

impl<T: Default> From<oneshot::Sender<T>> for Reply<T> {
    fn from(tx: oneshot::Sender<T>) -> Self {
        Self { tx: Some(tx) }
    }
}

impl<T: Default> Reply<T> {
    /// Responds to the actor message.
    pub fn reply(mut self, msg: T) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(msg);
        }
    }
}

impl<T: Default> Drop for Reply<T> {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(T::default());
        }
    }
}

#[macro_export]
/// A helper macro for deriving the [Message] trait.
macro_rules! derive_message {
    ($msg:ident, $output:ty) => {
        impl $crate::Message for $msg {
            type Output = $output;
        }
    };
}
