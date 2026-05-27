//! Timeline層
//!
//! LLMからのイベントストリームを受信し、登録されたHandlerにディスパッチします。
//! 通常はWorker経由で使用しますが、直接使用することも可能です。

use std::marker::PhantomData;

use super::event::*;
use crate::handler::*;

// =============================================================================
// Helpers
// =============================================================================

/// 1リクエスト内で受信した複数 UsageEvent をマージする。
/// 各フィールドについて新しい値が `Some` ならそれで上書き。
/// プロバイダによっては input/cache 系を最初の event だけに載せ、
/// output_tokens を後続 event で更新するため、最後の値だけを取るのではなく
/// フィールド単位で latest-non-None を取る。
fn merge_usage(acc: &mut UsageEvent, new: &UsageEvent) {
    if new.input_tokens.is_some() {
        acc.input_tokens = new.input_tokens;
    }
    if new.output_tokens.is_some() {
        acc.output_tokens = new.output_tokens;
    }
    if new.total_tokens.is_some() {
        acc.total_tokens = new.total_tokens;
    }
    if new.cache_read_input_tokens.is_some() {
        acc.cache_read_input_tokens = new.cache_read_input_tokens;
    }
    if new.cache_creation_input_tokens.is_some() {
        acc.cache_creation_input_tokens = new.cache_creation_input_tokens;
    }
}

// =============================================================================
// Type-erased Handler
// =============================================================================

/// 型消去された`Handler` trait
///
/// 各Handlerは独自のScope型を持つため、Timelineで保持するには型消去が必要です。
/// 通常は直接使用せず、`Timeline::on_text_block()`などのメソッド経由で
/// 自動的にラップされます。
pub trait ErasedHandler<K: Kind>: Send + Sync {
    /// イベントをディスパッチ
    fn dispatch(&mut self, event: &K::Event);
    /// スコープを開始（Block開始時）
    fn start_scope(&mut self);
    /// スコープを終了（Block終了時）
    fn end_scope(&mut self);
}

/// `Handler<K>`を`ErasedHandler<K>`として扱うためのラッパー
pub struct HandlerWrapper<H, K>
where
    H: Handler<K>,
    K: Kind,
{
    handler: H,
    scope: Option<H::Scope>,
    // fn() -> K は常にSend+Syncなので、Kの制約に関係なくSendを満たせる
    _kind: PhantomData<fn() -> K>,
}

impl<H, K> HandlerWrapper<H, K>
where
    H: Handler<K>,
    K: Kind,
{
    pub fn new(handler: H) -> Self {
        Self {
            handler,
            scope: None,
            _kind: PhantomData,
        }
    }
}

impl<H, K> ErasedHandler<K> for HandlerWrapper<H, K>
where
    H: Handler<K> + Send + Sync,
    K: Kind,
    H::Scope: Send + Sync,
{
    fn dispatch(&mut self, event: &K::Event) {
        if let Some(scope) = &mut self.scope {
            self.handler.on_event(scope, event);
        }
    }

    fn start_scope(&mut self) {
        self.scope = Some(H::Scope::default());
    }

    fn end_scope(&mut self) {
        self.scope = None;
    }
}

// =============================================================================
// Block Handler Registry
// =============================================================================

/// ブロックハンドラーの型消去trait
trait ErasedBlockHandler: Send + Sync {
    fn dispatch_start(&mut self, start: &BlockStart);
    fn dispatch_delta(&mut self, delta: &BlockDelta);
    fn dispatch_stop(&mut self, stop: &BlockStop);
    fn dispatch_abort(&mut self, abort: &BlockAbort);
    fn start_scope(&mut self);
    fn end_scope(&mut self);
    /// スコープがアクティブかどうか
    fn has_scope(&self) -> bool;
}

/// TextBlockKind用のラッパー
struct TextBlockHandlerWrapper<H>
where
    H: Handler<TextBlockKind>,
{
    handler: H,
    scope: Option<H::Scope>,
}

impl<H> TextBlockHandlerWrapper<H>
where
    H: Handler<TextBlockKind>,
{
    fn new(handler: H) -> Self {
        Self {
            handler,
            scope: None,
        }
    }
}

impl<H> ErasedBlockHandler for TextBlockHandlerWrapper<H>
where
    H: Handler<TextBlockKind> + Send + Sync,
    H::Scope: Send + Sync,
{
    fn dispatch_start(&mut self, start: &BlockStart) {
        if let Some(scope) = &mut self.scope {
            self.handler.on_event(
                scope,
                &TextBlockEvent::Start(TextBlockStart { index: start.index }),
            );
        }
    }

    fn dispatch_delta(&mut self, delta: &BlockDelta) {
        if let Some(scope) = &mut self.scope {
            if let DeltaContent::Text(text) = &delta.delta {
                self.handler
                    .on_event(scope, &TextBlockEvent::Delta(text.clone()));
            }
        }
    }

    fn dispatch_stop(&mut self, stop: &BlockStop) {
        if let Some(scope) = &mut self.scope {
            self.handler.on_event(
                scope,
                &TextBlockEvent::Stop(TextBlockStop {
                    index: stop.index,
                    stop_reason: stop.stop_reason.clone(),
                }),
            );
        }
    }

    fn dispatch_abort(&mut self, _abort: &BlockAbort) {
        // TextBlockはabortを特別扱いしない（スコープ終了のみ）
    }

    fn start_scope(&mut self) {
        self.scope = Some(H::Scope::default());
    }

    fn end_scope(&mut self) {
        self.scope = None;
    }

    fn has_scope(&self) -> bool {
        self.scope.is_some()
    }
}

/// ThinkingBlockKind用のラッパー
struct ThinkingBlockHandlerWrapper<H>
where
    H: Handler<ThinkingBlockKind>,
{
    handler: H,
    scope: Option<H::Scope>,
}

impl<H> ThinkingBlockHandlerWrapper<H>
where
    H: Handler<ThinkingBlockKind>,
{
    fn new(handler: H) -> Self {
        Self {
            handler,
            scope: None,
        }
    }
}

impl<H> ErasedBlockHandler for ThinkingBlockHandlerWrapper<H>
where
    H: Handler<ThinkingBlockKind> + Send + Sync,
    H::Scope: Send + Sync,
{
    fn dispatch_start(&mut self, start: &BlockStart) {
        if let Some(scope) = &mut self.scope {
            self.handler.on_event(
                scope,
                &ThinkingBlockEvent::Start(ThinkingBlockStart { index: start.index }),
            );
        }
    }

    fn dispatch_delta(&mut self, delta: &BlockDelta) {
        if let Some(scope) = &mut self.scope {
            if let DeltaContent::Thinking(text) = &delta.delta {
                self.handler
                    .on_event(scope, &ThinkingBlockEvent::Delta(text.clone()));
            }
        }
    }

    fn dispatch_stop(&mut self, stop: &BlockStop) {
        if let Some(scope) = &mut self.scope {
            self.handler.on_event(
                scope,
                &ThinkingBlockEvent::Stop(ThinkingBlockStop { index: stop.index }),
            );
        }
    }

    fn dispatch_abort(&mut self, _abort: &BlockAbort) {}

    fn start_scope(&mut self) {
        self.scope = Some(H::Scope::default());
    }

    fn end_scope(&mut self) {
        self.scope = None;
    }

    fn has_scope(&self) -> bool {
        self.scope.is_some()
    }
}

/// ToolUseBlockKind用のラッパー
struct ToolUseBlockHandlerWrapper<H>
where
    H: Handler<ToolUseBlockKind>,
{
    handler: H,
    scope: Option<H::Scope>,
    current_tool: Option<(String, String)>, // (id, name)
}

impl<H> ToolUseBlockHandlerWrapper<H>
where
    H: Handler<ToolUseBlockKind>,
{
    fn new(handler: H) -> Self {
        Self {
            handler,
            scope: None,
            current_tool: None,
        }
    }
}

impl<H> ErasedBlockHandler for ToolUseBlockHandlerWrapper<H>
where
    H: Handler<ToolUseBlockKind> + Send + Sync,
    H::Scope: Send + Sync,
{
    fn dispatch_start(&mut self, start: &BlockStart) {
        if let Some(scope) = &mut self.scope {
            if let BlockMetadata::ToolUse { id, name } = &start.metadata {
                self.current_tool = Some((id.clone(), name.clone()));
                self.handler.on_event(
                    scope,
                    &ToolUseBlockEvent::Start(ToolUseBlockStart {
                        index: start.index,
                        id: id.clone(),
                        name: name.clone(),
                    }),
                );
            }
        }
    }

    fn dispatch_delta(&mut self, delta: &BlockDelta) {
        if let Some(scope) = &mut self.scope {
            if let DeltaContent::InputJson(json) = &delta.delta {
                self.handler
                    .on_event(scope, &ToolUseBlockEvent::InputJsonDelta(json.clone()));
            }
        }
    }

    fn dispatch_stop(&mut self, stop: &BlockStop) {
        if let Some(scope) = &mut self.scope {
            if let Some((id, name)) = self.current_tool.take() {
                self.handler.on_event(
                    scope,
                    &ToolUseBlockEvent::Stop(ToolUseBlockStop {
                        index: stop.index,
                        id,
                        name,
                    }),
                );
            }
        }
    }

    fn dispatch_abort(&mut self, _abort: &BlockAbort) {
        self.current_tool = None;
    }

    fn start_scope(&mut self) {
        self.scope = Some(H::Scope::default());
    }

    fn end_scope(&mut self) {
        self.scope = None;
        self.current_tool = None;
    }

    fn has_scope(&self) -> bool {
        self.scope.is_some()
    }
}

// =============================================================================
// Timeline
// =============================================================================

/// イベントストリームの管理とハンドラへのディスパッチ
///
/// LLMからのイベントを受信し、登録されたハンドラに振り分けます。
/// ブロック系イベントはスコープ管理付きで処理されます。
///
/// # Examples
///
/// ```ignore
/// use llm_worker::{Timeline, Handler, TextBlockKind, TextBlockEvent};
///
/// struct MyHandler;
/// impl Handler<TextBlockKind> for MyHandler {
///     type Scope = String;
///     fn on_event(&mut self, buffer: &mut String, event: &TextBlockEvent) {
///         if let TextBlockEvent::Delta(text) = event {
///             buffer.push_str(text);
///         }
///     }
/// }
///
/// let mut timeline = Timeline::new();
/// timeline.on_text_block(MyHandler);
/// ```
///
/// # サポートするイベント種別
///
/// - **メタ系**: Usage, Ping, Status, Error
/// - **ブロック系**: TextBlock, ThinkingBlock, ToolUseBlock
pub struct Timeline {
    // Meta系ハンドラー
    usage_handlers: Vec<Box<dyn ErasedHandler<UsageKind>>>,
    ping_handlers: Vec<Box<dyn ErasedHandler<PingKind>>>,
    status_handlers: Vec<Box<dyn ErasedHandler<StatusKind>>>,
    error_handlers: Vec<Box<dyn ErasedHandler<ErrorKind>>>,
    reasoning_item_handlers: Vec<Box<dyn ErasedHandler<ReasoningItemKind>>>,

    // Block系ハンドラー（BlockTypeごとにグループ化）
    text_block_handlers: Vec<Box<dyn ErasedBlockHandler>>,
    thinking_block_handlers: Vec<Box<dyn ErasedBlockHandler>>,
    tool_use_block_handlers: Vec<Box<dyn ErasedBlockHandler>>,

    // 現在アクティブなブロック
    current_block: Option<BlockType>,

    // 1リクエスト内で受信した Usage event の集約バッファ。
    // Anthropic は message_start と message_delta、Gemini は各チャンクと、
    // 多くのプロバイダが複数 Usage を発行するため、リクエスト境界で
    // 1度だけ発火するためにここでマージする。flush_usage() で発火する。
    pending_usage: Option<UsageEvent>,
}

impl Default for Timeline {
    fn default() -> Self {
        Self::new()
    }
}

impl Timeline {
    pub fn new() -> Self {
        Self {
            usage_handlers: Vec::new(),
            ping_handlers: Vec::new(),
            status_handlers: Vec::new(),
            error_handlers: Vec::new(),
            reasoning_item_handlers: Vec::new(),
            text_block_handlers: Vec::new(),
            thinking_block_handlers: Vec::new(),
            tool_use_block_handlers: Vec::new(),
            current_block: None,
            pending_usage: None,
        }
    }

    // =========================================================================
    // Handler Registration
    // =========================================================================

    /// UsageKind用のHandlerを登録
    pub fn on_usage<H>(&mut self, handler: H) -> &mut Self
    where
        H: Handler<UsageKind> + Send + Sync + 'static,
        H::Scope: Send + Sync,
    {
        // Meta系はデフォルトでスコープを開始しておく
        let mut wrapper = HandlerWrapper::new(handler);
        wrapper.start_scope();
        self.usage_handlers.push(Box::new(wrapper));
        self
    }

    /// PingKind用のHandlerを登録
    pub fn on_ping<H>(&mut self, handler: H) -> &mut Self
    where
        H: Handler<PingKind> + Send + Sync + 'static,
        H::Scope: Send + Sync,
    {
        let mut wrapper = HandlerWrapper::new(handler);
        wrapper.start_scope();
        self.ping_handlers.push(Box::new(wrapper));
        self
    }

    /// StatusKind用のHandlerを登録
    pub fn on_status<H>(&mut self, handler: H) -> &mut Self
    where
        H: Handler<StatusKind> + Send + Sync + 'static,
        H::Scope: Send + Sync,
    {
        let mut wrapper = HandlerWrapper::new(handler);
        wrapper.start_scope();
        self.status_handlers.push(Box::new(wrapper));
        self
    }

    /// ErrorKind用のHandlerを登録
    pub fn on_error<H>(&mut self, handler: H) -> &mut Self
    where
        H: Handler<ErrorKind> + Send + Sync + 'static,
        H::Scope: Send + Sync,
    {
        let mut wrapper = HandlerWrapper::new(handler);
        wrapper.start_scope();
        self.error_handlers.push(Box::new(wrapper));
        self
    }

    /// `ReasoningItemKind` 用 Handler を登録
    pub fn on_reasoning_item<H>(&mut self, handler: H) -> &mut Self
    where
        H: Handler<ReasoningItemKind> + Send + Sync + 'static,
        H::Scope: Send + Sync,
    {
        let mut wrapper = HandlerWrapper::new(handler);
        wrapper.start_scope();
        self.reasoning_item_handlers.push(Box::new(wrapper));
        self
    }

    /// TextBlockKind用のHandlerを登録
    pub fn on_text_block<H>(&mut self, handler: H) -> &mut Self
    where
        H: Handler<TextBlockKind> + Send + Sync + 'static,
        H::Scope: Send + Sync,
    {
        self.text_block_handlers
            .push(Box::new(TextBlockHandlerWrapper::new(handler)));
        self
    }

    /// ThinkingBlockKind用のHandlerを登録
    pub fn on_thinking_block<H>(&mut self, handler: H) -> &mut Self
    where
        H: Handler<ThinkingBlockKind> + Send + Sync + 'static,
        H::Scope: Send + Sync,
    {
        self.thinking_block_handlers
            .push(Box::new(ThinkingBlockHandlerWrapper::new(handler)));
        self
    }

    /// ToolUseBlockKind用のHandlerを登録
    pub fn on_tool_use_block<H>(&mut self, handler: H) -> &mut Self
    where
        H: Handler<ToolUseBlockKind> + Send + Sync + 'static,
        H::Scope: Send + Sync,
    {
        self.tool_use_block_handlers
            .push(Box::new(ToolUseBlockHandlerWrapper::new(handler)));
        self
    }

    // =========================================================================
    // Event Dispatch
    // =========================================================================

    /// メインのディスパッチエントリポイント
    pub fn dispatch(&mut self, event: &Event) {
        match event {
            // Meta系: 即時ディスパッチ（登録順）
            Event::Usage(u) => self.dispatch_usage(u),
            Event::Ping(p) => self.dispatch_ping(p),
            Event::Status(s) => self.dispatch_status(s),
            Event::Error(e) => self.dispatch_error(e),
            // Observability-only event: stream trace records it before timeline dispatch.
            Event::UnhandledSse(_) => {}

            // Block系: スコープ管理しながらディスパッチ
            Event::BlockStart(s) => self.handle_block_start(s),
            Event::BlockDelta(d) => self.handle_block_delta(d),
            Event::BlockStop(s) => self.handle_block_stop(s),
            Event::BlockAbort(a) => self.handle_block_abort(a),

            // 完成済み reasoning item: 即時ディスパッチ
            Event::ReasoningItem(r) => self.dispatch_reasoning_item(r),
        }
    }

    /// Usage event を即時には dispatch せず、pending_usage にマージする。
    /// 1リクエスト内で複数の Usage event が来ても、ハンドラには 1 度だけ
    /// 最終値を渡したいため。flush_usage() で発火する。
    fn dispatch_usage(&mut self, event: &UsageEvent) {
        match &mut self.pending_usage {
            Some(acc) => merge_usage(acc, event),
            None => self.pending_usage = Some(event.clone()),
        }
    }

    /// pending_usage を usage_handlers に発火し、バッファをクリアする。
    /// 1リクエスト分のストリーム終了時に1回だけ呼ぶ想定。
    /// pending_usage が空ならば何もしない。
    pub fn flush_usage(&mut self) {
        if let Some(event) = self.pending_usage.take() {
            for handler in &mut self.usage_handlers {
                handler.dispatch(&event);
            }
        }
    }

    fn dispatch_ping(&mut self, event: &PingEvent) {
        for handler in &mut self.ping_handlers {
            handler.dispatch(event);
        }
    }

    fn dispatch_status(&mut self, event: &StatusEvent) {
        for handler in &mut self.status_handlers {
            handler.dispatch(event);
        }
    }

    fn dispatch_error(&mut self, event: &ErrorEvent) {
        for handler in &mut self.error_handlers {
            handler.dispatch(event);
        }
    }

    fn dispatch_reasoning_item(&mut self, event: &ReasoningItemEvent) {
        for handler in &mut self.reasoning_item_handlers {
            handler.dispatch(event);
        }
    }

    fn handle_block_start(&mut self, start: &BlockStart) {
        self.current_block = Some(start.block_type);

        let handlers = self.get_block_handlers_mut(start.block_type);
        for handler in handlers {
            handler.start_scope();
            handler.dispatch_start(start);
        }
    }

    fn handle_block_delta(&mut self, delta: &BlockDelta) {
        let block_type = delta.delta.block_type();

        // OpenAIなどのプロバイダはBlockStartを送らない場合があるため、
        // Deltaが来たときにスコープがなければ暗黙的に開始する
        if self.current_block.is_none() {
            self.current_block = Some(block_type);
        }

        let handlers = self.get_block_handlers_mut(block_type);
        for handler in handlers {
            // スコープがなければ暗黙的に開始
            if !handler.has_scope() {
                handler.start_scope();
            }
            handler.dispatch_delta(delta);
        }
    }

    fn handle_block_stop(&mut self, stop: &BlockStop) {
        let handlers = self.get_block_handlers_mut(stop.block_type);
        for handler in handlers {
            handler.dispatch_stop(stop);
            handler.end_scope();
        }
        self.current_block = None;
    }

    fn handle_block_abort(&mut self, abort: &BlockAbort) {
        let handlers = self.get_block_handlers_mut(abort.block_type);
        for handler in handlers {
            handler.dispatch_abort(abort);
            handler.end_scope();
        }
        self.current_block = None;
    }

    fn get_block_handlers_mut(
        &mut self,
        block_type: BlockType,
    ) -> &mut Vec<Box<dyn ErasedBlockHandler>> {
        match block_type {
            BlockType::Text => &mut self.text_block_handlers,
            BlockType::Thinking => &mut self.thinking_block_handlers,
            BlockType::ToolUse => &mut self.tool_use_block_handlers,
            BlockType::ToolResult => &mut self.text_block_handlers, // ToolResultはTextとして扱う
        }
    }

    /// 現在アクティブなブロックタイプを取得
    pub fn current_block(&self) -> Option<BlockType> {
        self.current_block
    }

    /// 現在アクティブなブロックを中断する
    ///
    /// キャンセルやエラー時に呼び出し、進行中のブロックに対して
    /// BlockAbortイベントを発火してスコープをクリーンアップする。
    pub fn abort_current_block(&mut self) {
        if let Some(block_type) = self.current_block {
            let abort = crate::timeline::event::BlockAbort {
                index: 0, // インデックスは不明なので0
                block_type,
                reason: "Cancelled".to_string(),
            };
            self.handle_block_abort(&abort);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn test_timeline_creation() {
        let timeline = Timeline::new();
        assert!(timeline.current_block().is_none());
    }

    #[test]
    fn unhandled_sse_is_ignored_by_timeline_handlers() {
        struct TestTextHandler {
            calls: Arc<Mutex<Vec<TextBlockEvent>>>,
        }

        impl Handler<TextBlockKind> for TestTextHandler {
            type Scope = ();
            fn on_event(&mut self, _scope: &mut (), event: &TextBlockEvent) {
                self.calls.lock().unwrap().push(event.clone());
            }
        }

        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut timeline = Timeline::new();
        timeline.on_text_block(TestTextHandler {
            calls: calls.clone(),
        });

        timeline.dispatch(&Event::UnhandledSse(UnhandledSseEvent {
            provider: "openai_responses".to_string(),
            event_type: "response.mystery".to_string(),
            data_preview: "{}".to_string(),
            data_len: 2,
        }));

        assert!(timeline.current_block().is_none());
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn test_meta_event_dispatch() {
        // シンプルなテスト用構造体
        struct TestUsageHandler {
            calls: Arc<Mutex<Vec<UsageEvent>>>,
        }

        impl Handler<UsageKind> for TestUsageHandler {
            type Scope = ();
            fn on_event(&mut self, _scope: &mut (), event: &UsageEvent) {
                self.calls.lock().unwrap().push(event.clone());
            }
        }

        let calls = Arc::new(Mutex::new(Vec::new()));
        let handler = TestUsageHandler {
            calls: calls.clone(),
        };

        let mut timeline = Timeline::new();
        timeline.on_usage(handler);

        timeline.dispatch(&Event::usage(100, 50));
        // pending_usage に積まれているだけなのでまだ未発火
        assert_eq!(calls.lock().unwrap().len(), 0);

        // flush で 1 度だけ発火
        timeline.flush_usage();
        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].input_tokens, Some(100));
    }

    #[test]
    fn test_usage_aggregation_and_flush() {
        struct TestUsageHandler {
            calls: Arc<Mutex<Vec<UsageEvent>>>,
        }
        impl Handler<UsageKind> for TestUsageHandler {
            type Scope = ();
            fn on_event(&mut self, _scope: &mut (), event: &UsageEvent) {
                self.calls.lock().unwrap().push(event.clone());
            }
        }

        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut timeline = Timeline::new();
        timeline.on_usage(TestUsageHandler {
            calls: calls.clone(),
        });

        // Anthropic 風: message_start で input + 暫定 output
        timeline.dispatch(&Event::Usage(UsageEvent {
            input_tokens: Some(409),
            output_tokens: Some(1),
            total_tokens: Some(410),
            cache_read_input_tokens: Some(0),
            cache_creation_input_tokens: Some(0),
        }));
        // message_delta で最終 output
        timeline.dispatch(&Event::Usage(UsageEvent {
            input_tokens: Some(409),
            output_tokens: Some(71),
            total_tokens: Some(480),
            cache_read_input_tokens: Some(0),
            cache_creation_input_tokens: Some(0),
        }));

        // 未 flush の段階では発火しない
        assert_eq!(calls.lock().unwrap().len(), 0);

        timeline.flush_usage();
        let recorded = calls.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].input_tokens, Some(409));
        assert_eq!(recorded[0].output_tokens, Some(71));

        // flush 後にもう一度 flush しても何も起きない
        drop(recorded);
        timeline.flush_usage();
        assert_eq!(calls.lock().unwrap().len(), 1);
    }
}
