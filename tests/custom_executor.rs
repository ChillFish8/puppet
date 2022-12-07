use puppet::{puppet_actor, ActorMailbox, Executor, Message};
use std::future::Future;

pub struct MyActor;

#[puppet_actor]
impl MyActor {
    #[puppet]
    async fn on_say_hello(&self, msg: SayHello) -> String {
        format!("Hello, {}!", msg.name)
    }
}

pub struct SayHello {
    name: String,
}
impl Message for SayHello {
    type Output = String;
}

pub struct CustomExecutor;
impl Executor for CustomExecutor {
    fn spawn(&self, fut: impl Future<Output = ()> + Send + 'static) {
        tokio::spawn(fut);
    }
}

#[tokio::test]
async fn run_actor_with_custom_executor() {
    let actor = MyActor;
    let mailbox: ActorMailbox<MyActor> = actor.spawn_actor_with("demo", 100, CustomExecutor).await;

    let message = SayHello {
        name: "Harri".to_string(),
    };
    let response = mailbox.send(message).await;
    println!("Got message back! {}", response);
}
