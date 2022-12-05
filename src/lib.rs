use futures::channel::oneshot;
use std::borrow::Cow;

pub use puppet_derive::puppet_actor;

// Not public API. Used by generated code.
#[doc(hidden)]
pub mod __private {
    pub use flume;
    pub use futures;
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

#[macro_export]
/// A helper macro for deriving the [Message] trait.
macro_rules! derive_message {
    ($msg:ident, $output:ty) => {
        impl $crate::Message for $msg {
            type Output = $output;
        }
    };
}
