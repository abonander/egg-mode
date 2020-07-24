// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

//! Structs and methods for working with direct messages.
//!
//! Note that direct message access requires a special permissions level above regular read/write
//! access. Your app must be configured to have "read, write, and direct message" access to use any
//! function in this module, even the read-only ones.
//!
//! In some sense, DMs are simpler than Tweets, because there are fewer ways to interact with them
//! and less metadata stored with them. However, there are also separate DM-specific capabilities
//! that are available, to allow users to create a structured conversation for things like
//! customer-service, interactive storytelling, etc. The extra DM-specific facilities are
//! documented in their respective builder functions on `DraftMessage`.
//!
//! ## Types
//!
//! * `DirectMessage`: The primary representation of a DM as retrieved from Twitter. Contains the
//!   types `DMEntities`/`Cta`/`QuickReply` as fields.
//! * `DMConversations`: Collection returned by `ActivityCursorIter::into_conversations` that sorts
//!   a user's direct messages based on who they're talking to.
//! * `DraftMessage`: As DMs have many optional parameters when creating them, this builder struct
//!   allows you to build up a DM before sending it.
//!
//! ## Functions
//!
//! * `list`: This creates a `ActivityCursorIter` struct to load a user's Direct Messages.
//! * `show`: This allows you to load a single DM from its ID.
//! * `delete`: This allows you to delete a DM from a user's own views. Note that it will not
//!   delete it entirely from the system; the recipient will still have a copy of the message.
//! * `mark_read`: This sends a read receipt for a given message to a given user. This also has the
//!   effect of clearing the message's "unread" status for the authenticated user.
//! * `indicate_typing`: This sends a typing indicator to a given user, to indicate that the
//!   authenticated user is typing or thinking of a response.

use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};

use chrono;
use serde::{Serialize, Deserialize};

use crate::common::*;
use crate::{auth, entities, error, links, media};
use crate::cursor::{self, ActivityCursor};
use crate::user::{self, UserID};
use crate::tweet::TweetSource;

mod fun;
pub(crate) mod raw;
pub mod welcome_messages;

pub use self::fun::*;

// TODO is this enough? i'm not sure if i want a field-by-field breakdown like with Tweet
/// Represents a single direct message.
#[derive(Debug)]
pub struct DirectMessage {
    /// Numeric ID for this DM.
    pub id: u64,
    /// UTC timestamp from when this DM was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// The text of the DM.
    pub text: String,
    /// Link, hashtag, and user mention information parsed out of the DM.
    pub entities: DMEntities,
    /// An image, gif, or video attachment, if present.
    pub attachment: Option<entities::MediaEntity>,
    /// A list of "call to action" buttons attached to the DM, if present.
    pub ctas: Option<Vec<Cta>>,
    /// A list of "Quick Replies" sent with this message to request structured input from the
    /// recipient.
    ///
    /// Note that there is no way to select a Quick Reply as a response in the public API; a
    /// `quick_reply_response` can only be populated if the Quick Reply was selected in the Twitter
    /// Web Client, or Twitter for iOS/Android.
    pub quick_replies: Option<Vec<QuickReply>>,
    /// The `metadata` accompanying a Quick Reply, if the sender selected a Quick Reply for their
    /// response.
    pub quick_reply_response: Option<String>,
    /// The ID of the user who sent the DM.
    ///
    /// To load full user information for the sender or recipient, use `user::show`. Note that
    /// Twitter may show a message with a user that doesn't exist if that user has been suspended
    /// or has deleted their account.
    pub sender_id: u64,
    /// The app that sent this direct message.
    ///
    /// Source app information is only available for messages sent by the authorized user. For
    /// received messages written by other users, this field will be `None`.
    pub source_app: Option<TweetSource>,
    /// The ID of the user who received the DM.
    ///
    /// To load full user information for the sender or recipient, use `user::show`. Note that
    /// Twitter may show a message with a user that doesn't exist if that user has been suspended
    /// or has deleted their account.
    pub recipient_id: u64,
}

impl From<raw::SingleEvent> for DirectMessage {
    fn from(ev: raw::SingleEvent) -> DirectMessage {
        let raw::SingleEvent { event, apps } = ev;
        let raw: raw::RawDirectMessage = event.as_raw_dm();
        raw.into_dm(&apps)
    }
}

impl From<raw::EventCursor> for Vec<DirectMessage> {
    fn from(evs: raw::EventCursor) -> Vec<DirectMessage> {
        let raw::EventCursor { events, apps, .. } = evs;
        let mut ret = vec![];

        for ev in events {
            let raw: raw::RawDirectMessage = ev.as_raw_dm();
            ret.push(raw.into_dm(&apps));
        }

        ret
    }
}

impl From<raw::EventCursor> for ActivityCursor<DirectMessage> {
    fn from(mut curs: raw::EventCursor) -> ActivityCursor<DirectMessage> {
        let next_cursor = curs.next_cursor.take();

        ActivityCursor {
            next_cursor,
            items: curs.into(),
        }
    }
}

impl cursor::ActivityItem for DirectMessage {
    type Cursor = raw::EventCursor;
}

/// Container for URL, hashtag, and mention information associated with a direct message.
///
/// As far as entities are concerned, a DM can contain nearly everything a tweet can. The only
/// thing that isn't present here is the "extended media" that would be on the tweet's
/// `extended_entities` field. A user can attach a single picture to a DM, but if that is present,
/// it will be available in the `attachments` field of the original `DirectMessage` struct and not
/// in the entities.
///
/// For all other fields, if the message contains no hashtags, financial symbols ("cashtags"),
/// links, or mentions, those corresponding fields will be empty.
#[derive(Debug, Deserialize)]
pub struct DMEntities {
    /// Collection of hashtags parsed from the DM.
    pub hashtags: Vec<entities::HashtagEntity>,
    /// Collection of financial symbols, or "cashtags", parsed from the DM.
    pub symbols: Vec<entities::HashtagEntity>,
    /// Collection of URLs parsed from the DM.
    pub urls: Vec<entities::UrlEntity>,
    /// Collection of user mentions parsed from the DM.
    pub user_mentions: Vec<entities::MentionEntity>,
}

/// A "call to action" added as a button to a direct message.
///
/// Buttons allow you to attach additional URLs as "calls to action" for the recipient of the
/// message. For more information, see the `cta_button` function on [`DraftMessage`].
///
/// [`DraftMessage`]: struct.DraftMessage.html
#[derive(Debug, Deserialize)]
pub struct Cta {
    /// The label shown to the user for the CTA.
    pub label: String,
    /// The `t.co` URL that the user should navigate to if they click this CTA.
    pub tco_url: String,
    /// The URL given for the CTA, that could be displayed if needed.
    pub url: String,
}

/// A version of `Cta` without `tco_url` to be used in `DraftMessage`.
struct DraftCta {
    label: String,
    url: String,
}

/// A Quick Reply attached to a message to request structured input from a user.
///
/// For more information about Quick Replies, see the `quick_reply_option` function on
/// [`DraftMessage`].
///
/// [`DraftMessage`]: struct.DraftMessage.html
#[derive(Debug, Serialize, Deserialize)]
pub struct QuickReply {
    /// The label shown to the user. When the user selects this Quick Reply, the label will be sent
    /// as the `text` of the reply message.
    pub label: String,
    /// An optional description that accompanies a Quick Reply.
    pub description: Option<String>,
    /// Metadata that accompanies this Quick Reply. Metadata is not shown to the user, but is
    /// available in the `quick_reply_response` when the user selects it.
    pub metadata: String,
}

/// Wrapper around a collection of direct messages, sorted by their recipient.
///
/// The mapping exposed here is from a User ID to a listing of direct messages between the
/// authenticated user and that user. Messages sent from the authenticated user to themself are
/// sorted under the user's own ID. This map is returned by the `into_conversations` adapter on
/// [`ActivityCursorIter`].
///
/// [`ActivityCursorIter`]: ../cursor/struct.ActivityCursorIter.html
pub type DMConversations = HashMap<u64, Vec<DirectMessage>>;

/// Represents a direct message before it is sent.
///
/// Because there are several optional items you can add to a DM, this struct allows you to add or
/// skip them using a builder-style struct, much like with `DraftTweet`.
///
/// To begin drafting a direct message, start by calling `new` with the message text and the User
/// ID of the recipient:
///
/// ```no_run
/// use egg_mode::direct::DraftMessage;
///
/// # let recipient: egg_mode::user::TwitterUser = unimplemented!();
/// let message = DraftMessage::new("hey, what's up?", recipient.id);
/// ```
///
/// As-is, the draft won't do anything until you call `send` to send it:
///
/// ```no_run
/// # #[tokio::main]
/// # async fn main() {
/// # let message: egg_mode::direct::DraftMessage = unimplemented!();
/// # let token: egg_mode::Token = unimplemented!();
/// message.send(&token).await.unwrap();
/// # }
/// ```
///
/// In between creating the draft and sending it, you can use any of the other adapter functions to
/// add other information to the message. See the documentation for those functions for details.
pub struct DraftMessage {
    text: Cow<'static, str>,
    recipient: UserID,
    quick_reply_options: VecDeque<QuickReply>,
    cta_buttons: VecDeque<DraftCta>,
    media_attachment: Option<media::MediaId>,
}

impl DraftMessage {
    /// Creates a new `DraftMessage` with the given text, to be sent to the given recipient.
    ///
    /// Note that while this accepts a `UserID`, Twitter only accepts a numeric ID to denote the
    /// recipient. If you pass this function a string Screen Name, a separate user lookup will
    /// occur when you `send` this message. To avoid this extra lookup, use a numeric ID (or the
    /// `UserID::ID` variant of `UserID`) when creating a `DraftMessage`.
    pub fn new(text: impl Into<Cow<'static, str>>, recipient: impl Into<UserID>) -> DraftMessage {
        DraftMessage {
            text: text.into(),
            recipient: recipient.into(),
            quick_reply_options: VecDeque::new(),
            cta_buttons: VecDeque::new(),
            media_attachment: None,
        }
    }

    /// Adds an Option-type Quick Reply to this draft message.
    ///
    /// Quick Replies allow you to request structured input from the other user. They'll have the
    /// opportunity to select from the options you add to the message when you send it. If they
    /// select one of the given options, its `metadata` will be given in the response in the
    /// `quick_reply_response` field.
    ///
    /// Note that while `description` is optional in this call, Twitter will not send the message
    /// if only some of the given Quick Replies have `description` fields.
    ///
    /// The fields here have the following length restrictions:
    ///
    /// * `label` has a maximum of 36 characters, including spaces.
    /// * `metadata` has a maximum of 1000 characters, including spaces.
    /// * `description` has a maximum of 72 characters, including spaces.
    ///
    /// There is a maximum of 20 Quick Reply Options on a single Direct Message. If you try to add
    /// more, the oldest one will be removed.
    ///
    /// Users can only respond to Quick Replies in the Twitter Web Client, and Twitter for
    /// iOS/Android.
    ///
    /// It is not possible to respond to a Quick Reply sent to yourself, though Twitter will
    /// register the options in the message it returns.
    pub fn quick_reply_option(
        mut self,
        label: impl Into<String>,
        metadata: impl Into<String>,
        description: Option<String>
    ) -> Self {
        if self.quick_reply_options.len() == 20 {
            self.quick_reply_options.pop_front();
        }
        self.quick_reply_options.push_back(QuickReply {
            label: label.into(),
            metadata: metadata.into(),
            description,
        });
        self
    }

    /// Adds a "Call To Action" button to the message.
    ///
    /// Buttons allow you to add up to three links to a message. These links act as an extension to
    /// the message rather than embedding the URLs into the message text itself. If a [Web Intent
    /// link] is used as the URL, they can also be used to bounce users back into the Twitter UI to
    /// perform some action.
    ///
    /// [Web Intent link]: https://developer.twitter.com/en/docs/twitter-for-websites/web-intents/overview
    ///
    /// The `label` has a length limit of 36 characters.
    ///
    /// There is a maximum of 3 CTA Buttons on a single Direct Message. If you try to add more, the
    /// oldest one will be removed.
    pub fn cta_button(mut self, label: impl Into<String>, url: impl Into<String>) -> Self {
        if self.cta_buttons.is_empty() {
            self.cta_buttons.reserve_exact(3);
        } else if self.cta_buttons.len() == 3 {
            self.cta_buttons.pop_front();
        }
        self.cta_buttons.push_back(DraftCta {
            label: label.into(),
            url: url.into(),
        });
        self
    }

    /// Add the given media to this message.
    ///
    /// The `MediaId` needs to have been uploaded via [`media::upload_media_for_dm`]. Twitter
    /// requires DM-specific media categories for media that will be attached to Direct Messages.
    /// In addition, there's an extra setting available for media attached to Direct Messages. For
    /// more information, see the documentation for `upload_media_for_dm`.
    ///
    /// [`media::upload_media_for_dm`]: ../media/fn.upload_media_for_dm.html
    pub fn attach_media(self, media_id: media::MediaId) -> Self {
        DraftMessage {
            media_attachment: Some(media_id),
            ..self
        }
    }

    /// Sends this direct message using the given `Token`.
    ///
    /// The recipient must allow DMs from the authenticated user for this to be successful. In
    /// practice, this means that the recipient must either follow the authenticated user, or they must
    /// have the "allow DMs from anyone" setting enabled. As the latter setting has no visibility on
    /// the API, there may be situations where you can't verify the recipient's ability to receive the
    /// requested DM beforehand.
    ///
    /// If the message was successfully sent, this function will return the `DirectMessage` that
    /// was just sent.
    pub async fn send(self, token: &auth::Token) -> Result<Response<DirectMessage>, error::Error> {
        let recipient_id = match self.recipient {
            UserID::ID(id) => id,
            UserID::ScreenName(name) => {
                let user = user::show(name, token).await?;
                user.id
            }
        };
        let mut message_data = serde_json::json!({
            "text": self.text
        });
        if !self.quick_reply_options.is_empty() {
            message_data.as_object_mut().unwrap().insert("quick_reply".into(), serde_json::json!({
                "type": "options",
                "options": self.quick_reply_options
            }));
        }
        if !self.cta_buttons.is_empty() {
            message_data.as_object_mut().unwrap().insert("ctas".into(),
                self.cta_buttons.into_iter().map(|b| serde_json::json!({
                    "type": "web_url",
                    "label": b.label,
                    "url": b.url,
                })).collect::<Vec<_>>().into()
            );
        }
        if let Some(media_id) = self.media_attachment {
            message_data.as_object_mut().unwrap().insert("attachment".into(), serde_json::json!({
                "type": "media",
                "media": {
                    "id": media_id.0
                }
            }));
        }

        let message = serde_json::json!({
            "event": {
                "type": "message_create",
                "message_create": {
                    "target": {
                        "recipient_id": recipient_id
                    },
                    "message_data": message_data
                }
            }
        });
        let req = post_json(links::direct::SEND, token, message);
        let resp: Response<raw::SingleEvent> = request_with_json_response(req).await?;
        Ok(Response::into(resp))
    }
}
