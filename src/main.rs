use anyhow::Context as _;
use once_cell::sync::Lazy;
use poise::serenity_prelude::{
    ButtonStyle, CacheHttp, Color, CreateActionRow, Http, InteractionResponseType, InteractionType,
    MessageComponentInteraction,
};
use poise::{serenity_prelude as serenity, BoxFuture, Event, FrameworkContext};
use serde::{Deserialize, Serialize};
use shuttle_persist::PersistInstance;
use shuttle_poise::ShuttlePoise;
use shuttle_secrets::SecretStore;

//Static poll buttons as they are the same and do not need to be recreated every time
static POLL_BUTTONS: Lazy<CreateActionRow> = Lazy::new(|| {
    let mut row = CreateActionRow::default();

    row.create_button(|b| {
        b.custom_id("poll_yes")
            .label("Yes!")
            .style(ButtonStyle::Success)
    })
    .create_button(|b| {
        b.custom_id("poll_no")
            .label("No!")
            .style(ButtonStyle::Danger)
    })
    .create_button(|b| {
        b.custom_id("poll_view")
            .label("View Results")
            .style(ButtonStyle::Primary)
    });

    row
});

#[derive(Clone)]
struct Data {
    persist: PersistInstance,
} // User data, which is stored and accessible in all command invocations

#[derive(Serialize, Deserialize, Clone)]
struct Poll {
    title: String,
    description: String,
    reason_to_vote_yes: String,
    reason_to_vote_no: String,
    yes_votes: Vec<PollVote>,
    no_votes: Vec<PollVote>,
}

#[derive(Serialize, Deserialize, Clone)]
//u64 = UserId
struct PollVote(u64);

type Error = Box<dyn std::error::Error + Send + Sync>;
type Context<'a> = poise::Context<'a, Data, Error>;

//Creates a poll
#[poise::command(slash_command)]
async fn poll(
    ctx: Context<'_>,
    title: String,
    description: String,
    reason_to_vote_yes: String,
    reason_to_vote_no: String,
) -> Result<(), Error> {
    let persist = ctx.data().clone().persist;

    let reply = ctx
        .send(|r| {
            r.embed(|e| {
                e.title(title.clone())
                    .description(description.clone())
                    .color(Color::from_rgb(0, 255, 0))
                    .field("Yes", reason_to_vote_yes.clone(), true)
                    .field("No", reason_to_vote_no.clone(), true)
            })
            .components(|c| c.add_action_row(POLL_BUTTONS.clone()))
        })
        .await?;

    let message = reply.message().await?;
    persist.save(
        &message.id.to_string(),
        Poll {
            title,
            description,
            reason_to_vote_yes,
            reason_to_vote_no,
            yes_votes: Vec::new(),
            no_votes: Vec::new(),
        },
    )?;
    Ok(())
}

///Responds to a component interaction with ephemeral text
async fn eph_text(
    interaction: &MessageComponentInteraction,
    text: impl Into<String>,
    http: &Http,
) -> Result<(), Error> {
    interaction
        .create_interaction_response(http, |r| {
            r.kind(InteractionResponseType::ChannelMessageWithSource)
                .interaction_response_data(|d| d.ephemeral(true).content(text.into()))
        })
        .await?;
    Ok(())
}

///Check if a user has voted
fn get_voted(
    component_interaction: &MessageComponentInteraction,
    yes_votes: &[PollVote],
    no_votes: &[PollVote],
) -> bool {
    yes_votes
        .iter()
        .any(|v| component_interaction.user.id.0 == v.0)
        || no_votes
            .iter()
            .any(|v| component_interaction.user.id.0 == v.0)
}

#[shuttle_runtime::main]
async fn poise(
    #[shuttle_secrets::Secrets] secret_store: SecretStore,
    #[shuttle_persist::Persist] persist: PersistInstance,
) -> ShuttlePoise<Data, Error> {
    // Get the discord token set in `Secrets.toml`
    let discord_token = secret_store
        .get("DISCORD_TOKEN")
        .context("'DISCORD_TOKEN' was not found")?;

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: vec![poll()],
            event_handler: |ctx: &serenity::Context,
                            event,
                            fw_ctx: FrameworkContext<Data, Error>,
                            _|
             -> BoxFuture<'_, Result<(), Error>> {
                Box::pin(async move {
                    if let Event::InteractionCreate { interaction } = event {
                        if interaction.kind() != InteractionType::MessageComponent {
                            return Ok(());
                        }

                        let component_interaction = interaction.as_message_component().unwrap();
                        let component_data = component_interaction.clone().data;

                        let poll_id = &component_interaction.message.id.to_string();
                        let mut poll: Poll = fw_ctx.user_data.persist.load(poll_id)?;

                        if !component_data.custom_id.starts_with("poll_") {
                            return eph_text(component_interaction, "Unknown id", ctx.http()).await;
                        }

                        if get_voted(component_interaction, &poll.yes_votes, &poll.no_votes) {
                            return eph_text(
                                component_interaction,
                                "You already voted!",
                                ctx.http(),
                            )
                            .await;
                        }

                        match component_data.custom_id.as_str() {
                            "poll_yes" => {
                                eph_text(component_interaction, "You voted yes!", ctx.http())
                                    .await?;

                                poll.yes_votes
                                    .append(&mut vec![PollVote(component_interaction.user.id.0)])
                            }
                            "poll_no" => {
                                eph_text(component_interaction, "You voted no!", ctx.http())
                                    .await?;

                                poll.no_votes
                                    .append(&mut vec![PollVote(component_interaction.user.id.0)])
                            }
                            "poll_view" => {
                                return eph_text(
                                    component_interaction,
                                    format!(
                                        "Yes: {} No: {}",
                                        poll.yes_votes.len(),
                                        poll.no_votes.len()
                                    ),
                                    ctx.http(),
                                )
                                .await;
                            }
                            _ => {}
                        }

                        fw_ctx.user_data.clone().persist.save(poll_id, poll)?;
                    }
                    Ok(())
                })
            },
            ..Default::default()
        })
        .token(discord_token)
        .intents(serenity::GatewayIntents::non_privileged())
        .setup(|ctx, _ready, framework| {
            Box::pin(async move {
                poise::builtins::register_globally(ctx, &framework.options().commands).await?;
                Ok(Data { persist })
            })
        })
        .build()
        .await
        .map_err(shuttle_runtime::CustomError::new)?;

    Ok(framework.into())
}
