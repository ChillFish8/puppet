# Puppet - A simple actor framework

Puppet is yet another actor framework built without any boxing or dynamic dispatch of items, instead it uses
a small macro to essentially generate the boilerplate for an enum-based system.

## Features
- Generic-aware, which means you can have generic actors return generic messages.
- Box-less, no requirement for `async_trait` or boxing for dynamic dispatch.
- Linter friendly, your editors should be able to easily infer the return types from all exposed methods.
- Sync and Async mailbox methods.

## Basic Example
Let's create a small actor for creating hello messages...

```rust
use puppet::{puppet_actor, ActorMailbox, Message};

pub struct MyActor;

#[puppet_actor]
impl MyActor {
    #[puppet]
    async fn on_say_hello(&self, msg: SayHello) -> String {
        format!("Hello, {}!", msg.name)
    }
}

pub struct SayHello {
    name: String
}
impl Message for SayHello {
    type Output = String;
}

#[tokio::main]
async fn main() {
    // Create the actor.
    let actor = MyActor;
    
    // Spawn it on the current runtime, which returns to us a mailbox which
    // we can use to communicate with the actor.
    let mailbox: ActorMailbox<MyActor> = actor.spawn_actor().await;

    let message = SayHello {
        name: "Harri".to_string(),
    };
    
    // Send a message to the actor and wait for a response.
    let response = mailbox.send(message).await;
    println!("Got message back! {}", response);
}
```

## Generic Example
Now what if we want to do some more advanced things with our actors? Well luckily for us, we can use generics.

```rust
use puppet::{puppet_actor, ActorMailbox, Message};

pub struct AppenderService<T: Clone> {
    seen_data: Vec<T>,
}

#[puppet_actor]
impl<T> AppenderService<T>
where
    // The additional `Send` and `'static` bounds are required due to the nature
    // of the actor running as a tokio task which has it's own requirements.
    T: Clone + Send + 'static,
{
    fn new() -> Self {
        Self {
            seen_data: Vec::new(),
        }
    }

    #[puppet]
    async fn on_append_and_return(&mut self, msg: AppendAndReturn<T>) -> Vec<T> {
        self.seen_data.push(msg.value);
        self.seen_data.clone()
    }
}

#[derive(Clone)]
pub struct AppendAndReturn<T: Clone> {
    value: T,
}
impl<T> Message for AppendAndReturn<T>
where
    T: Clone,
{
    type Output = Vec<T>;
}

#[tokio::main]
async fn main() {
    let actor = AppenderService::<String>::new();
    let mailbox: ActorMailbox<AppenderService<String>> = actor.spawn_actor().await;

    let message = AppendAndReturn {
        value: "Harri".to_string(),
    };

    for _ in 0..3 {
        let response = mailbox.send(message.clone()).await;
        println!("Got values: {:?}", response);
    }
}
```