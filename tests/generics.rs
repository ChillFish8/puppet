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

#[tokio::test]
async fn run_basic_actor() {
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
