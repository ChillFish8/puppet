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
    let mailbox: ActorMailbox<MyActor> = actor.spawn_actor_with_name("Test").await;

    assert_eq!(mailbox.name(), "Test");
}
