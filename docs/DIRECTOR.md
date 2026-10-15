# Morning director

A local model may speak for other clubs on a morning the player advances one day. Rust decides what is legal. The model picks among those options and writes the sentence a person would say. This is not the MCP server: MCP is one agent playing the user's club. The director is the rest of the world, and only the part of it the player will actually read.

The code lives in three places.

| Path | Role |
|------|------|
| `src-tauri/src/director.rs` | The HTTP call. Runs before the day lock. |
| `src-tauri/crates/ofm_core/src/director.rs` | The slate, the parse, and applying a bid at the fee Rust already computed. |
| `src-tauri/crates/ofm_core/src/director_voice.rs` | Language, the named speaker, and the one extra sentence of the morning. |
| `src-tauri/crates/ofm_core/src/transfers/director.rs` | The predicate a bid must pass, shared with the ordinary market. |

`consult` is called from `application/time_advancement.rs` (`advance_time_with_mode`) and from `commands/time.rs` (one day). A skip of many days does not come through either, so it never calls the model. The call happens before the game mutex is taken. A missing server must not hold the save.

## What the model is allowed to do

It receives a JSON slate and returns JSON. For each club it echoes that club's short id and one option id, or `"pass"`, plus a `reason` (`squad_need`, `contract`, `ambition`, `rivalry`) and `text`. An optional `voice` string is the one non-transfer sentence.

It cannot:

- invent a fee, a player, or a club (unknown ids write nothing, and the ordinary market may still speak that day)
- put a digit in a sentence (the sentence is dropped; the bid can still stand if the rest parsed)
- be asked on a day the player skips
- block the day if LM Studio is down

The fee stored on the offer is the fee printed on the option, from `suggested_incoming_fee`. An option only appears when `apply_bid` would accept that fee: window, identity, registration, already in talks, cooldown, queue, finance, transfer budget, wage, reputation deficit, squad depth.

A pass writes a journal line and no offer. The same day, `evaluate_transfer_market` skips a user-owned target whose buyer already has a journal line. AI-to-AI transfers still run. If the reply is empty or the server is offline, the journal is not written and the heuristic market runs as usual.

## Orbit, as it is today

`ORBIT_CLUBS` is 3. Each club gets up to 3 options. Clubs must be in the active scope, not the user's club, with transfer budget and finance above zero. Sort is: the next scheduled opponent first, then reputation descending. Only clubs that can legally bid someone survive the cut.

That is not "the clubs I am fighting in the table". A rich foreign club that can pay beats a poorer neighbour in the user's league. The orbit the player actually means is the clubs near them in their own competition (reputation neighbours while the table is still 0-0), plus the next opponent, plus a club already in talks. More than three of the right clubs is fine. All twenty, every morning, is where the call gets slow and the sentences go generic. That selection is not built yet.

One decision per club. If the model emits three choices with the same `club_id`, the first is kept.

## The sentence, and who says it

`text` is two or three sentences in the game's language, about a fact already on the option (position, contract end, squad depth) and about memory when that club is in it. The game language is `language` in the app `settings.json` (`com.sturdyrobot.openfootmanager` under the XDG data dir). The prompt names that language in English (`"es"` becomes `Spanish`). An unknown code is English. Person and club names stay as stored.

The speaker is a person the save already has:

- the club's `Manager`, by `team_id` (the user's own manager is `Game.manager`, everyone else is in `Game.managers`)
- an interim manager is labelled assistant
- otherwise a staff member with role `AssistantManager` and a non-empty `display_name`

There is no chairman record. Board mail uses the translation key `be.sender.boardOfDirectors`. Do not invent a president's name.

The inbox card is `transfer_interest_{player_id}`, one thread per player, on purpose. A later bid replaces the visible sentence. Earlier offers stay on the player; the template's `n` is how many clubs are still in. The sender field is the person's name. The role and the wrapper (`be.msg.transferInterest.directorManager` or `directorAssistant`) are translated. The quote itself is the model's prose, passed through as `{{quote}}`.

Digits are banned so the model cannot smuggle a fee. It will spell years ("twenty twenty eight"). That is the rule working, not a second fee. The limit is 480 characters. `max_tokens` on the HTTP call is 700, because three clubs of two sentences plus a voice used to die at 400 with `finish_reason: length`. A cut reply is not valid JSON; `parse_director_choices` keeps every choice object that closed and drops the one the cut landed in. Short ids (`a`, `a1`) exist so a full reply fits.

## The one extra voice

At most one, and only when the morning has a fact to hang it on. Priority:

1. **Monday, and a real bid in the journal from the last seven days.** The paper (`director_paper_{date}`) quotes that club's speaker about that player. The invented weekly rumour is suppressed that Monday (`paper_pending`), so the gazette does not contradict the bid.
2. **Otherwise, the morning after the user's league fixture.** The opponent's manager speaks. The score is Rust's, in the headline params. The quote must not repeat it, because of the digit ban. Friendlies do not count; `FixtureCompetition::League` does.
3. **Otherwise, a contract letter the day is about to send**, and only when the user's assistant has a display name. The twelve-month stage does not send a letter, matching `should_generate_contract_concern`. The message id prefix is `contract_concern_{player_id}_`.

`Game.director_voice` is `#[serde(skip)]`. It is set in the morning and consumed by `publish_director_voice` at the end of `process_day_common`, before the clock advances. It is not a column. A crash between the two `update_game` calls drops the sentence; the bid journal is the part that is saved.

No Monday, no league result yet, and no due contract letter means `voice` is `""`. That is a quiet morning, not a failed call.

## Memory

`Game.director_journal` is the last eight decisions: date, club, player, `bid` or `pass`, reason. The next slate shows that tail, with names rather than ids. It is `game_meta.director_journal_json`, migration `v056`. Old saves load as `[]`. `CURRENT_SAVE_FORMAT_VERSION` stayed put; an empty journal is the right default for a save from before this column.

## The model server

LM Studio on `http://127.0.0.1:1234/v1`, model id `google/gemma-4-e4b`, unless the env says otherwise.

| Variable | Effect |
|----------|--------|
| `OFM_DIRECTOR_URL` | API root. Default `http://127.0.0.1:1234/v1`. |
| `OFM_DIRECTOR_MODEL` | Model id from `lms ls`. |
| `OFM_DIRECTOR=0` or `off` | No call. |
| `OFM_DIRECTOR=1` or `on` | Call, including under `cfg(test)`. Tests are off unless this is set. |
| `OFM_DIRECTOR_TIMEOUT_SECS` | Default 30. Connect timeout is 500 ms. |

Gemma and Qwen put the answer in `reasoning_content` and leave `content` empty unless the request sends `reasoning_effort: "none"`. `reasoning.enabled`, `thinking`, and `chat_template_kwargs` did not stop that. Temperature is 0.2. A schema HTTP 400 is retried once without the schema. Connection refused marks the process offline so the next mornings do not each wait.

The game log is UTC. The LM Studio log is local time. `truncated = 0` in the llama slot line means the context window was fine; `finish_reason: length` means `max_tokens`. The server log truncates request bodies, so the slate you want is in `choice_user_prompt`, not in that file.

Load the model with a context that covers the prompt. A 605-token slate plus a 225-token reply is a normal morning. Something near 8k is enough.

## What not to bolt on next

The same seam is "facts in, speech out". The model should not gain a path that writes a result, a wage, a table position, or an injury. Those already have generators. A new sentence belongs on a message or article the player already receives, for a club in the orbit, in the game language, with the speaker's name taken from the save.

Still open, in the order they change what a season would measure:

- Choose the orbit from the user's competition, not from world reputation.
- One club answering three times is discarded after the first. The prompt already says one decision per club; the apply side enforces it.
- Pre-match preview and board letters are still templates. There is no named chairman to sign the board ones.
- The old spotlight API (`spotlight_brief`, `apply_director_intents`, model-invented fees) is still in `director.rs` and still tested. `consult` does not call it. Do not wire it back up.

## Tests

```bash
cargo test --manifest-path src-tauri/Cargo.toml -p ofm_core --lib director
```

That runs `director::tests` and `director_voice::tests`. A new behaviour needs a failing test first, in the same file, with a Given/When/Then comment. The HTTP client is not part of those tests.
