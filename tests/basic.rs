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
    name: String,
}
impl Message for SayHello {
    type Output = String;
}

#[tokio::test]
async fn run_basic_actor() {
    let actor = MyActor;
    let mailbox: ActorMailbox<MyActor> = actor.spawn_actor().await;

    let message = SayHello {
        name: "Harri".to_string(),
    };
    let response = mailbox.send(message).await;
    println!("Got message back! {}", response);
}
