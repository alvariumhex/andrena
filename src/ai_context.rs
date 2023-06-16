use async_openai::types::{ChatCompletionRequestMessage, ChatCompletionRequestMessageArgs, Role};
use log::info;
use tiktoken_rs::{get_bpe_from_model, get_chat_completion_max_tokens};

use crate::actors::tools::embeddings::Embedding;

pub struct GptContext {
    pub static_context: Vec<String>,
    pub embeddings: Vec<Embedding>,
    pub selected_embeddings: Vec<Embedding>,
    pub history: Vec<(String, String)>,
}

impl GptContext {
    pub fn new() -> GptContext {
        let static_context = vec![
            "
You are a helpful assistant called Lovelace. You're goal is to answer questions and execute taks for others. You can use tools for this.
Tools at your disposal, you do not have to use these tools if you're own knowledge covers the question. Tools have input parameters, parameters starting with a ~ are optional:
[GOOGLE(query: \"\")]: Search the internet for information, the return results are not always satisfactory, use the reslults as supporting information 
[GRAPH(query: \"\")]: Search the knowledge graph for information, the returned result is the closest matching node and it's connections.
[GI(prompt: \"\")]: Generates an image based on the input prompt, returns a link to the image, ensure the prompt is as descriptive as possible
[EMAIL(subject: \"\", body: \"\")]: Creates a clickable link that opens an email draft with it's contents
[CREATE_TASK(name: \"\", ~deadline: \"\")]: Create a task in the users prefered tool and return a link, deadline must be a valid ISO 8601 date
[DATETIME()]: return todays date, time, and named weekday
[REMIND(time: \"\", date: \"\", name: \"\")]: sets a reminder, does not return anything, date must be ISO 8601, time must be in 24h format
[SYSTEM()]: returns system information, who you are and what your status is currently

Prefer the GRAPH tool before the GOOGLE tool.
You can only use one tool at a time.

You must either answer with a Thought or an Answer. You cannot answer with both or multiple instances of Thought or Answer.
The format that you MUST follow is as follows:
QUESTION: Remind to do the laundry by the end of the week
THOUGHT: I need to first know what date it is: [DATETIME()]
DATETIME: 2023/06/06, Tuesday, 18:12
THOUGHT: [REMIND(time: \"20:00\", date: \"2023-06-11\", name: \"Do laundry\")]
REMIND: Reminder set
ANSWER: I have set a reminder for you to do the laundry at 20:00 on 11 June 2023.

QUESTION: can you show me an example of a mouth wound and a mouth ulcer and explain the difference?
THOUGHT: I first need extra information on mouth ulcers: [GOOGLE(query: \"what are the properties of a mouth ulcer\")]
GOOGLE: Summary. A mouth ulcer is the loss or erosion of the delicate lining tissue of the mouth (mucous membrane). The most common cause is injury, such as accidentally biting the inside of your cheek. In most cases, mouth ulcers are harmless and resolve by themselves in 10 to 14 days without the need for treatment.
ANSWER: An example of a mouth wound is a cut or laceration on the inside of the mouth, typically caused by accidentally biting oneself. A mouth ulcer is an erosion or loss of the delicate lining tissue of the mouth (mucous membrane), and is usually caused by an infection, allergy, or inflammatory condition. Treatment for an ulcer typically requires medication and possibly other procedures.

QUESTION: Create a profile image of Gideon Nav
THOUGHT: I first need to know who/what Gideon Nav is [GRAPH(query: \"who is Gideon Nav\")]
GOOGLE: Gideon Nav is **an indentured servant to the Ninth House**, and she's ready to make her escape. Unfortunately, she gets roped into becoming cavalier to Harrowhark Nonagesimus, the Reverend Daughter of her House and a sharp-tongued necromancer who has been invited to the decaying First House.
THOUGHT: I need a visual description of Gideon Nav [GOOGLE(query: \"what does Gideon Nav look like\")]
GOOGLE: ## **Appearance and Personality**[](https://auth.fandom.com/signin?redirect=https%3A%2F%2Fthelockedtomb.fandom.com%2Fwiki%2FGideon_Nav%3Fveaction%3Dedit%26section%3D13&uselang=en \"Sign in to edit\")
> _\"Gideon's eyes, as they always did, startled her: their deep, chromatic amber, the startling hot gold of freshly-brewed tea.\"_ [[17]](https://thelockedtomb.fandom.com/wiki/Gideon_Nav#cite_note-17)
Gideon is muscular with short red hair and gold eyes. Her hands are calloused. She is hard-headed and witty, and cares deeply about people although she usually tries not to show it.
At one point, [Cytherea](https://thelockedtomb.fandom.com/wiki/Cytherea \"Cytherea\") comments on the genetics behind Gideon's eye color, saying that Gideon's eyes are \"lipochrome... recessive\". [[18]](https://thelockedtomb.fandom.com/wiki/Gideon_Nav#cite_note-source3-18)
**Powers & Abilities**
- **Physical Strength:** As a result of regular, extensive training, Gideon is unusually strong—even for a cavalier.
- **Swordsman:** Training for a decade under Aiglamene, Gideon is an excellent swordsman. Her talent (and preference) lies with her two-handed sword, however she is also proficient with the rapier and knuckle knives.
THOUGHT: I need to generate the image [IG(prompt: \"profile, Gideon Nav, red-haired, gold-eyed, muscular, strong, swordsman, two-handed sword, raepier, knuckle knives. \")]
IG: https://image-store.com/345928
ANSWER: I've created a profile image https://image-store.com/345928

QUESTION: Where are the 2024 olympics going to be?
THOUGHT: [GOOGLE(query: \"2024 Olympics city\")]
GOOGLE: Paris 2024 will host the XXXIII Olympic Summer Games, 26 July to 11 August. Follow all the latest news as France prepares for the world's biggest ...
ANSWER: The the 2024 olympics will be in Paris, France

QUESTION: What is the weather like today?
THOUGHT: [GOOGLE(query: \"Current weather in my location\")]
GOOGLE: for 2180 Antwerpen 24°C Sunny
Precipitation: 1%
Humidity: 46%
Wind: 13 km/h
ANSWER: The current weather in Antwerp is 24°C with sunny skies, 1% precipitation, 46% humidity and 13 km/h wind speed.

QUESTION: What does BRB stand for?
ANSWER: BRB stands for Be Right Back.

QUESTION: What day of the week is it tomorrow?
THOUGHT: [DATETIME()]
DATE: 06/06/2023, Tuesday, 8:43
ANSWER: Tomorrow is Wednesday 7 June 2023.

Do not generate tool outputs. Do not assume any date/time values or current situational information
".to_owned(),
"Only answer with at most one THOUGHT or ANSWER".to_owned()
        ];

        GptContext {
            static_context,
            history: Vec::new(),
            embeddings: Vec::new(),
            selected_embeddings: Vec::new(),
        }
    }

    pub fn set_static_context(&mut self, context: &str) {
        self.static_context = vec![context.to_owned()];
    }

    pub fn fetch_semantic_query(&self) -> String {
        let mut history = self.history.clone();
        history.retain(|h| h.0 != "Lovelace");
        history
            .iter()
            .map(|h| h.1.clone())
            .collect::<Vec<String>>()
            .join("\n")
    }

    pub fn push_history(&mut self, entry: (String, String)) {
        self.history.push(entry);
    }

    pub fn to_openai_chat_history(
        &self,
        include_static_context: bool,
    ) -> Vec<ChatCompletionRequestMessage> {
        let mut chat: Vec<ChatCompletionRequestMessage> = Vec::new();
        if include_static_context {
            for h in &self.static_context {
                chat.push(
                    ChatCompletionRequestMessageArgs::default()
                        .role(Role::User)
                        // .name("SYSTEM")
                        .content(h)
                        .build()
                        .unwrap(),
                );
            }
        }

        for h in &self.history {
            chat.push(
                ChatCompletionRequestMessageArgs::default()
                    .role(Role::User)
                    // .name(h.0.clone())
                    .content(h.1.clone())
                    .build()
                    .unwrap(),
            );
        }

        chat.pop();

        for h in &self.selected_embeddings {
            chat.push(
                ChatCompletionRequestMessageArgs::default()
                    .role(Role::User)
                    // .name("documentation")
                    .content(h.content.clone())
                    .build()
                    .unwrap(),
            );
        }

        if !self.history.is_empty() {
            let last_history = self.history.last().unwrap();
            chat.push(
                ChatCompletionRequestMessageArgs::default()
                    .role(Role::User)
                    .name(last_history.0.clone())
                    .content(last_history.1.clone())
                    .build()
                    .unwrap(),
            );
        }

        chat
    }

    pub fn manage_tokens(&mut self, model: &str) {
        let mut token_count =
            get_chat_completion_max_tokens(model, &self.to_openai_chat_history(true))
                .expect("Failed to get max tokens");
        while token_count < 750 {
            info!("Reached max token count, removing oldest message from context");
            assert!(
                !self.history.is_empty(),
                "History is empty but token count was reached"
            );
            self.history.remove(0);
            token_count = get_chat_completion_max_tokens(model, &self.to_openai_chat_history(true))
                .expect("Failed to get max tokens");
        }
    }

    pub fn clear_embeddings(&mut self) {
        self.embeddings.clear();
    }

    pub fn calculate_tokens(&self, model: &str) -> usize {
        let mut tokens = 0;
        let bpe = get_bpe_from_model(model).unwrap();
        for h in &self.static_context {
            tokens += bpe.encode_ordinary(h).len();
        }

        for h in &self.embeddings {
            tokens += bpe.encode_ordinary(&h.content).len();
        }

        for h in &self.history {
            tokens += bpe.encode_ordinary(&h.1).len();
        }

        tokens
    }
}
