use serenity::{model::prelude::interaction::application_command::CommandDataOption, builder::CreateApplicationCommand};

use crate::GptContext;

pub fn run(options: &[CommandDataOption], gpt: &mut GptContext) -> String {
    gpt.context.drain(2..);
    "Context cleared!".to_owned()
}

pub fn register(command: &mut CreateApplicationCommand) -> &mut CreateApplicationCommand {
    command.name("clear_context").description("Clear the chat context")
}
