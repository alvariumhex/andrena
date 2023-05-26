use std::{
    sync::mpsc,
    thread::{self, JoinHandle},
};

use ractor::{Actor, ActorProcessingErr, ActorRef, RpcReplyPort};
use rust_bert::pipelines::sentence_embeddings::{
    SentenceEmbeddingsBuilder, SentenceEmbeddingsModelType,
};
use tokio::sync::oneshot;

pub struct Embedding<T>
where
    T: Embeddable,
{
    pub vectors: Vec<(String, Vec<f32>)>,
    pub source: T,
}

pub trait Embeddable {
    fn human_readable_source(&self) -> String;
    fn short_description(&self) -> String;
    fn long_description(&self) -> String;
    fn get_chunks(&self, size: usize) -> Vec<String>;
}

pub struct EmbeddingGenerator;

// TODO I'm not smart enough to make this generic over the Embeddable trait
pub enum EmbeddingGeneratorMessage {
    Generate(
        Vec<Box<dyn Embeddable + Send + Sync>>,
        usize,
        RpcReplyPort<Vec<(String, Vec<f32>)>>,
    ),
    Query(String, RpcReplyPort<Vec<f32>>),
}

impl ractor::Message for EmbeddingGeneratorMessage {}

type SyncEmbeddingMessage = (Vec<String>, oneshot::Sender<Vec<Vec<f32>>>);
pub struct EmbeddingGeneratorState {
    sender: mpsc::SyncSender<SyncEmbeddingMessage>,
}

impl EmbeddingGeneratorState {
    pub fn spawn() -> (JoinHandle<()>, Self) {
        let (sender, receiver) = mpsc::sync_channel(100);
        let handle = thread::spawn(move || Self::runner(&receiver));

        (handle, Self { sender })
    }

    fn runner(receiver: &mpsc::Receiver<SyncEmbeddingMessage>) {
        let model = SentenceEmbeddingsBuilder::remote(SentenceEmbeddingsModelType::AllMiniLmL12V2)
            .create_model()
            .expect("Could not create model");

        while let Ok((texts, sender)) = receiver.recv() {
            let embeddings = model.encode(&texts).unwrap();
            sender.send(embeddings).unwrap();
        }
    }

    pub async fn predict(&self, sentences: Vec<String>) -> Vec<Vec<f32>> {
        let (sender, receiver) = oneshot::channel();
        self.sender.send((sentences, sender)).unwrap();
        receiver.await.unwrap()
    }
}

#[async_trait::async_trait]
impl Actor for EmbeddingGenerator {
    type Msg = EmbeddingGeneratorMessage;
    type State = EmbeddingGeneratorState;
    type Arguments = ();

    async fn pre_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        _args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        let (_handle, state) = EmbeddingGeneratorState::spawn();
        Ok(state)
    }

    async fn handle(
        &self,
        _myself: ActorRef<Self::Msg>,
        msg: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match msg {
            EmbeddingGeneratorMessage::Generate(embeddables, size, reply_port) => {
                let mut embeddings = Vec::new();
                for embeddable in embeddables {
                    let chunks = embeddable.get_chunks(size);
                    let vectors = state.predict(chunks.clone()).await;
                    let results: Vec<(String, Vec<f32>)> =
                        chunks.into_iter().zip(vectors).collect();
                    embeddings.extend(results);
                }

                reply_port.send(embeddings).unwrap();
            }
            EmbeddingGeneratorMessage::Query(string, reply_port) => {
                let vectors = state.predict(vec![string]).await;
                reply_port.send(vectors[0].clone()).unwrap();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use ractor::call;

    use crate::actors::tools::github::{GitHubFile, GitHubRepo};

    use super::*;

    #[tokio::test]
    async fn state_start() {
        let (_handle, model) = EmbeddingGeneratorState::spawn();
        assert_eq!(model.predict(vec!["Hello".to_string()]).await.len(), 1);
    }

    #[tokio::test]
    async fn actor_start() {
        let (actor, _) = Actor::spawn(None, EmbeddingGenerator, ()).await.unwrap();
        assert!(
            actor.get_status() == ractor::ActorStatus::Starting
                || actor.get_status() == ractor::ActorStatus::Running
        );
    }

    #[tokio::test]
    async fn generate_embedding_from_gh_file() {
        let (actor, _) = Actor::spawn(None, EmbeddingGenerator, ()).await.unwrap();
        let embedding = GitHubFile {
            path: "src/actors/tools/github.rs".to_string(),
            content: FILE_CONTENT.to_string(),
            metadata: HashMap::new(),
            repo: GitHubRepo {
                owner: "rust-bert".to_string(),
                name: "rust-bert".to_string(),
                branch: "master".to_string(),
            },
        };

        let embedding = Box::new(embedding);

        let embedding = call!(
            actor,
            EmbeddingGeneratorMessage::Generate,
            vec![embedding],
            300
        )
        .unwrap();
        assert_eq!(embedding.len(), 3);
    }

    // static example file content
    static FILE_CONTENT: &str = "
# Check Greengrass core device status<a name=\"device-status\"></a>

Greengrass core devices report the status of their software components to AWS IoT Greengrass. You can check the health summary of each device, and you can check the status of each component on each device.

Core devices have the following health statuses:
+ `HEALTHY` – The AWS IoT Greengrass Core software and all components run without issue on the core device.
+ `UNHEALTHY` – The AWS IoT Greengrass Core software or a component is in an error state on the core device.

**Note**  
AWS IoT Greengrass relies on individual devices to send status updates to the AWS Cloud. If the AWS IoT Greengrass Core software isn't running on the device, or if device isn't connected to the AWS Cloud, then the reported status of that device might not reflect its current status. The status timestamp indicates when the device status was last updated.  
Core devices send status updates at the following times:  
When the AWS IoT Greengrass Core software starts
When the core device receives a deployment from the AWS Cloud
When the status of any component on the core device becomes `BROKEN`
At a [regular interval that you can configure](greengrass-nucleus-component.md#greengrass-nucleus-component-configuration-fss), which defaults to 24 hours
For AWS IoT Greengrass Core v2.7.0, the core device sends status updates when local deployment and cloud deployment occurs

**Topics**
+ [Check health of a core device](#check-core-device-health-status)
+ [Check health of a core device group](#check-core-device-group-health-status)
+ [Check core device component status](#check-core-device-component-status)

## Check health of a core device<a name=\"check-core-device-health-status\"></a>

You can check the status of individual core devices.

**To check the status of a core device (AWS CLI)**
+ Run the following command to retrieve the status of a device. Replace *coreDeviceName* with the name of the core device to query.

  ```
  aws greengrassv2 get-core-device --core-device-thing-name coreDeviceName
  ```

  The response contains information about the core device, including its status.

## Check health of a core device group<a name=\"check-core-device-group-health-status\"></a>

You can check the status of a group of core devices (a thing group).

**To check the status of a group of devices (AWS CLI)**
+ Run the following command to retrieve the status of multiple core devices. Replace the ARN in the command with the ARN of the thing group to query.

  ```
  aws greengrassv2 list-core-devices --thing-group-arn \"arn:aws:iot:region:account-id:thinggroup/thingGroupName\"
  ```

  The response contains the list of core devices in the thing group. Each entry in the list contains the status of the core device.

## Check core device component status<a name=\"check-core-device-component-status\"></a>

You can check the status, such as lifecycle state, of the software components on a core device. For more information about component lifecycle states, see [Develop AWS IoT Greengrass components](develop-greengrass-components.md).

**To check the status of components on a core device (AWS CLI)**
+ Run the following command to retrieve the status of the components on a core device. Replace *coreDeviceName* with the name of the core device to query.

  ```
  aws greengrassv2 list-installed-components --core-device-thing-name coreDeviceName
  ```

  The response contains the list of components that run on the core device. Each entry in the list contains the lifecycle state of the component, including how current the status of the data is and when the Greengrass core device last sent a message containing a certain component to the cloud. The response will also include the most recent deployment source that brought the component to the Greengrass core device.
**Note**  
This command retrieves a paginated list of the components that a Greengrass core device runs. By default, this list doesn't include components that are deployed as dependencies of other components. You can include dependencies in the response by setting the `topologyFilter` parameter to `ALL`.
";
}
