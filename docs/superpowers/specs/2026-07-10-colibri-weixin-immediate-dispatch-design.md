# Colibri Weixin Immediate Dispatch Design

## Goal

Remove all attachment-waiting and complementary-message aggregation from the
Weixin channel while preserving the separation between network reception and
agent execution.

## Message Flow

The receive loop remains the sole owner of Weixin getupdates polling. Every
parsed InboundMessage is handled immediately in receive order:

1. a text-only message that belongs to an active permission waiter is routed to
   that waiter;
2. every other message is published directly to the bounded worker queue;
3. the single worker invokes the gateway handler and sends its reply.

There are no debounce timers, pending attachment candidates, cross-poll merges,
or same-sender complementary checks. If Weixin emits image and text as separate
messages, Colibri intentionally executes two agent turns.

## Preserved Decoupling

The queue remains bounded by MAX_PENDING_MESSAGES and the existing
stop-aware publisher continues to use timed puts. The receive loop can keep
polling while the worker is waiting for a model or tool. One worker serializes
agent execution and avoids concurrent access to the same gateway session.

Worker errors, stop-event propagation, non-blocking stop-sentinel publication,
and bounded worker join remain unchanged. Finalization no longer closes a
batcher because no pending timers or messages exist outside the queue.

## Permission Replies

The registered text-waiter mechanism is not attachment aggregation and remains
unchanged. It allows the receive loop to deliver y, s, p, or n replies while the
agent worker is blocked waiting for permission, without starting a competing
getupdates loop.

## Configuration and Cleanup

Remove channels.weixin.message_debounce_seconds from WeixinChannelConfig, all
example TOML files, tests, and documentation. Old user configuration containing
the removed key is no longer accepted; Colibri does not retain compatibility
code for deleted configuration.

Delete _PendingInbound, _ComplementaryMessageBatcher,
_messages_are_complementary, _message_has_text_and_media, and
_merge_inbound_messages. Keep _publish_work as the queue boundary between
reception and execution.

## Tests

Replace aggregation tests with a regression test proving text and image from
separate polls are delivered as two ordered agent turns. Preserve tests for
text ordering, permission reply routing, bounded queue shutdown, media parsing,
and gateway execution.
