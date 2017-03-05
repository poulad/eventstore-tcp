use std::fmt;
use std::error::Error;
use std::ops::Range;
use client_messages::{self, OperationResult, WriteEvents};
use {Message, AsMessageWrite, StreamVersion, LogPosition};

impl<'a> From<WriteEvents<'a>> for Message {
    fn from(we: WriteEvents<'a>) -> Self {
        use client_messages_ext::WriteEventsExt;
        Message::WriteEvents(we.into_owned())
    }
}

fn range_from_parts(start: i32, end: i32) -> Range<StreamVersion> {
    StreamVersion::from_i32(start)..StreamVersion::from_i32(end + 1)
}

fn range_to_parts(r: &Range<StreamVersion>) -> (i32, i32) {
    let start: i32 = r.start.into();
    let end: i32 = r.end.into();
    (start, end - 1)
}

/// Successful response to `Message::WriteEvents`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteEventsCompleted {
    /// The event number range assigned to the written events
    pub event_numbers: Range<StreamVersion>,

    /// Position for `$all` query for one of the written events, perhaps the first?
    pub prepare_position: Option<LogPosition>,

    /// These can be used to locate last written event from the `$all` stream
    pub commit_position: Option<LogPosition>,
}

impl AsMessageWrite<client_messages::WriteEventsCompleted<'static>> for WriteEventsCompleted {
    fn as_message_write(&self) -> client_messages::WriteEventsCompleted<'static> {
        let parts = range_to_parts(&self.event_numbers);
        client_messages::WriteEventsCompleted {
            result: Some(client_messages::OperationResult::Success),
            message: None,
            first_event_number: parts.0,
            last_event_number: parts.1,
            prepare_position: self.prepare_position.map(|x| x.into()),
            commit_position: self.commit_position.map(|x| x.into()),
        }
    }
}

impl<'a> From<client_messages::WriteEventsCompleted<'a>> for Message {
    fn from(wec: client_messages::WriteEventsCompleted<'a>) -> Self {
        use client_messages::OperationResult::*;

        // FIXME: can panic
        let res = match wec.result {
            Some(Success) => {
                Ok(WriteEventsCompleted {
                    event_numbers: range_from_parts(wec.first_event_number, wec.last_event_number),
                    // unsure if these should be:
                    //  * separate (instead of newtype for tuple)
                    //  * options
                    prepare_position: wec.prepare_position.map(|x| x.into()),
                    commit_position: wec.commit_position.map(|x| x.into()),
                })
            }
            Some(other) => Err(other.into()),
            None => panic!("OperationResult was not found in the received message"),
        };

        Message::WriteEventsCompleted(res)
    }
}

/// Like `OperationResult` on the wire but does not have a success value. Explains the reason for
/// failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteEventsFailure {
    /// Server failed to process the request before timeout
    PrepareTimeout,
    /// Server timed out while awaiting commit to be processed
    CommitTimeout,
    /// Server timed out while awaiting for a forwarded request to complete
    ForwardTimeout,
    /// Optimistic locking failure; stream version was not the expected
    WrongExpectedVersion,
    /// Stream has been deleted
    StreamDeleted,
    /// No authentication provided or insufficient permissions to a stream
    AccessDenied,
}

impl Copy for WriteEventsFailure {}

impl WriteEventsFailure {
    /// Return `true` if the operation failed in a transient way that might be resolved by
    /// retrying.
    pub fn is_transient(&self) -> bool {
        use WriteEventsFailure::*;
        match *self {
            PrepareTimeout | CommitTimeout | ForwardTimeout => true,
            _ => false
        }
    }
}

impl AsMessageWrite<client_messages::WriteEventsCompleted<'static>> for WriteEventsFailure {
    fn as_message_write(&self) -> client_messages::WriteEventsCompleted<'static> {
        client_messages::WriteEventsCompleted {
            result: Some(self.clone().into()),
            message: None,
            first_event_number: -1,
            last_event_number: -1,
            prepare_position: None,
            commit_position: None,
        }
    }
}

impl From<OperationResult> for WriteEventsFailure {
    fn from(or: OperationResult) -> Self {
        use self::OperationResult::*;

        match or {
            Success => unreachable!(),
            InvalidTransaction => unreachable!(),
            PrepareTimeout => WriteEventsFailure::PrepareTimeout,
            CommitTimeout => WriteEventsFailure::CommitTimeout,
            ForwardTimeout => WriteEventsFailure::ForwardTimeout,
            WrongExpectedVersion => WriteEventsFailure::WrongExpectedVersion,
            StreamDeleted => WriteEventsFailure::StreamDeleted,
            AccessDenied => WriteEventsFailure::AccessDenied,
        }
    }
}

impl Into<OperationResult> for WriteEventsFailure {
    fn into(self) -> OperationResult {
        use WriteEventsFailure::*;
        match self {
            PrepareTimeout => OperationResult::PrepareTimeout,
            CommitTimeout => OperationResult::CommitTimeout,
            ForwardTimeout => OperationResult::ForwardTimeout,
            WrongExpectedVersion => OperationResult::WrongExpectedVersion,
            StreamDeleted => OperationResult::StreamDeleted,
            AccessDenied => OperationResult::AccessDenied
        }
    }
}

impl fmt::Display for WriteEventsFailure {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "{}", self.description())
    }
}

impl Error for WriteEventsFailure {
    fn description(&self) -> &str {
        use WriteEventsFailure::*;
        match *self {
            PrepareTimeout => "Internal server timeout, should be retried",
            CommitTimeout => "Internal server timeout, should be retried",
            ForwardTimeout => "Server timed out while awaiting response to forwarded request, should be retried",
            WrongExpectedVersion => "Stream version was not expected, optimistic locking failure",
            StreamDeleted => "Stream had been deleted",
            AccessDenied => "Access to stream was denied"
        }
    }
}