# SDK Token Usage Investigation

Follow-up to [research/05](05-context-and-cost-tracking.md). Answers the open questions from Section 11.1 about how `claude-agent-sdk-rs` v0.6.3 exposes token usage, using SDK source analysis and real session log data.

---

## 1. Does Message::Assistant Expose a Message ID?

**Yes.** `AssistantMessageInner` has an `id: Option<String>` field:

```rust
// claude-agent-sdk-rs/src/types/messages.rs:120-140
pub struct AssistantMessageInner {
    pub content: Vec<ContentBlock>,
    pub model: Option<String>,
    pub id: Option<String>,           // ← message ID for deduplication
    pub stop_reason: Option<String>,
    pub usage: Option<serde_json::Value>,
    pub error: Option<AssistantMessageError>,
}
```

Access path: `Message::Assistant(m) → m.message.id`.

From raw session data, all content blocks within the same API turn share the same message ID (e.g., `msg_01Sh9ixGrA2JqFtNyhWqP7Tr` appears on the thinking block, text block, and tool_use block of a single turn). This is the correct deduplication key.

---

## 2. Are Per-Message Usage Fields Cumulative or Incremental?

**Input tokens are per-API-call (full conversation history). Output tokens are unreliable snapshots.**

### Input side: full history each turn

Each `Message::Assistant`'s usage reflects the **complete input** to that API call — system prompt + all prior messages + tool definitions. The total input is:

```
total_input = input_tokens + cache_read_input_tokens + cache_creation_input_tokens
```

This grows monotonically across turns as the conversation accumulates history:

```
Turn  1: total_input= 21,415   (initial: system prompt + first user message)
Turn  2: total_input= 23,313   (+1,898 from tool results added to history)
Turn  3: total_input= 31,040   (+7,727)
Turn  4: total_input= 31,422   (+382)
...
Turn 50: total_input= 60,145   (full conversation at end of step)
```

**Summing input across turns = total billed input.** Each API call is independently charged for its full input. The Result message's cumulative input totals equal the sum of per-turn inputs:

| Metric | Sum of per-turn | Result message | Match? |
|--------|----------------|----------------|--------|
| `input_tokens` | 51 | 51 | Yes |
| `cache_read_input_tokens` | 2,162,931 | 2,162,931 | Yes |
| `cache_creation_input_tokens` | 53,050 | 53,050 | Yes |

*(Data from session `20260207-050025`, task 13 step 1, 50 turns)*

### Output side: unreliable partial snapshots

Per-turn `output_tokens` does NOT reflect the actual output for that turn. Values are typically tiny (1, 2, 3, 5) regardless of actual output length. Only the Result message has the true cumulative output count:

| Metric | Sum of per-turn | Result message |
|--------|----------------|----------------|
| `output_tokens` | 419 | 9,257 |

*(Same step — 50 turns. Per-turn sum captures <5% of actual output.)*

This pattern is consistent across all sessions analyzed:

| Session/Task | Per-turn output sum | Result output | Ratio |
|-------------|-------------------|---------------|-------|
| 050025/13-s1 | 419 | 9,257 | 4.5% |
| 150937/16 | 66 | 8,482 | 0.8% |
| 150937/17 | 118 | 12,150 | 1.0% |
| 150937/18 | 57 | 10,890 | 0.5% |
| 150937/19 | 142 | 15,274 | 0.9% |
| 150937/20 | 203 | 14,059 | 1.4% |
| 041033/7 | 88 | 2,764 | 3.2% |
| 041033/8 | 24,599 | 10,192 | 241% (!) |

Task 041033/8 is an outlier where the per-turn sum *exceeds* the Result — likely due to the deduplication issue (same usage counted multiple times from duplicate Assistant messages, accumulating bogus output values).

### Why output_tokens is unreliable

From raw Claude Code session logs (archive), the SDK delivers one `Message::Assistant` per content block. Within a single turn (same message ID):

```
msg_01HXGmSWDUC7kQbMMAcBYigN: 4 blocks
  ['thinking'] output_tokens=1      ← partial/placeholder
  ['tool_use'] output_tokens=1      ← partial/placeholder
  ['tool_use'] output_tokens=1      ← partial/placeholder
  ['tool_use'] output_tokens=211    ← sometimes the last block updates

msg_01Sh9ixGrA2JqFtNyhWqP7Tr: 3 blocks
  ['thinking'] output_tokens=1      ← partial/placeholder
  ['text']     output_tokens=1      ← partial/placeholder
  ['tool_use'] output_tokens=1      ← never updates (stays at placeholder)
```

The output_tokens on intermediate blocks is a streaming placeholder. Sometimes the final block updates it, sometimes not. The SDK does not guarantee an accurate output count on any individual `Message::Assistant`.

---

## 3. Does the SDK Send Duplicate Assistant Messages?

**Yes.** The SDK sends one `Message::Assistant` per content block within a single API turn. A turn that produces `[thinking, text, tool_use, tool_use]` generates **4 separate Assistant messages**, all with:
- The same `message.id`
- Identical `usage` fields (on all but sometimes the last block, which may have updated `output_tokens`)
- One content block each

### Duplication counts from real sessions

| Turn content | Duplicate count |
|-------------|----------------|
| text + tool_use | 2x |
| text + 2×tool_use | 3x |
| text + 4×tool_use + 3×tool_use (parallel) | 5-8x |
| single tool_use (after tool results) | 1x |

Session `20260207-150937` task 16 shows turns reported up to **8x** (8 content blocks in one turn).

### Impact on TokenAccumulator

The current `TokenAccumulator.add_from_event()` adds usage from every `TokenUsage` event. Without deduplication, the accumulator over-counts by the duplication factor. For a step with 12 turns averaging 3x duplication, the accumulator would report ~36x worth of usage instead of ~12x.

The current code uses value-equality deduplication (skipping identical consecutive usages), which works in most cases because identical values within a turn are consecutive. But this breaks when:
1. A turn's last block has different `output_tokens` (breaks the equality check)
2. Two different turns happen to have identical usage (unlikely but possible)

**Message ID deduplication is the correct approach.**

---

## 4. What Does the Result Message Represent?

The `ResultMessage.usage` contains **cumulative billing totals** across all API calls in the conversation:

```json
{
  "input_tokens": 51,        // sum of per-call uncached input
  "output_tokens": 9257,     // sum of ALL output (including thinking, tool JSON)
  "cache_read_input_tokens": 2162931,    // sum of per-call cache reads
  "cache_creation_input_tokens": 53050   // sum of per-call cache writes
}
```

### Key properties

- **Input fields:** Equal to the sum of deduplicated per-turn values (verified across 8 steps in 3 sessions, exact match in 6/8, close match in 2/8 where SDK may have internal turns not surfaced as Assistant messages)
- **Output field:** The authoritative total output. Much larger than per-turn output sums because per-turn values are unreliable placeholders
- **cost_usd:** Also on the Result message, authoritative total cost

### Result vs. last-turn for context tracking

The last turn's input tokens give the current context size (since input = full conversation history). But the last turn's output tokens are just a small placeholder. For context tracking:

```
context_used ≈ last_turn_total_input + estimated_output
```

Where `estimated_output` can't come from per-turn values. Options:
1. Use only the input side: `last_turn_total_input / 200,000 * 100` — underestimates but safe
2. Track cumulative output ourselves from content block text lengths (approximate)
3. Accept the underestimate (output is typically <20% of context in tool-heavy workflows)

---

## 5. Recommendations for rloop

### Context tracking (Approach A from research/05, refined)

Use the last `Message::Assistant`'s usage for context percentage, **but only the input side**:

```rust
struct ContextTracker {
    last_message_id: Option<String>,
    last_total_input: u64,  // input + cache_read + cache_create
}

impl ContextTracker {
    fn update(&mut self, message_id: &str, usage: &serde_json::Value) {
        if self.last_message_id.as_deref() == Some(message_id) {
            return; // same turn, same usage
        }
        self.last_message_id = Some(message_id.to_string());
        let input = get_u64(usage, "input_tokens");
        let cache_read = get_u64(usage, "cache_read_input_tokens");
        let cache_create = get_u64(usage, "cache_creation_input_tokens");
        self.last_total_input = input + cache_read + cache_create;
    }

    fn percentage(&self) -> f32 {
        (self.last_total_input as f64 / 200_000.0 * 100.0) as f32
    }
}
```

This slightly underestimates (excludes output tokens from context calculation) but is safe and correct. The underestimate is typically 5-15%.

### Token accumulation for billing

Use the Result message's values directly. Do not sum per-turn values:

```rust
// From Message::Result:
let cost = res.total_cost_usd.unwrap_or(0.0);
let input = get_u64(usage, "input_tokens");
let output = get_u64(usage, "output_tokens");
// etc.
```

If live running totals are needed during streaming (before Result arrives), use a deduplicated accumulator with message ID, but only for input-side tokens. Accept that live output totals will be inaccurate.

### Deduplication

Always deduplicate by `message.id`:

```rust
if m.message.id.as_deref() == last_seen_id.as_deref() {
    // Skip — same turn, duplicate content block
    continue;
}
last_seen_id = m.message.id.clone();
```

---

## 6. Data Sources

All findings based on:

- **SDK source:** `claude-agent-sdk-rs` v0.6.3 from cargo registry
- **Session logs analyzed:**
  - `20260207-050025.jsonl` — 480 lines, 4 steps across 3 tasks (13, 14, 15)
  - `20260207-150937.jsonl` — 5 steps across tasks 16-20
  - `20260207-041033.jsonl` — 996 lines, 6 tasks (7-12)
- **Raw CC session log:** `archive/rstep-era/sessions/4a75e42d-*.jsonl` — 1601 lines with raw message IDs and content block breakdown
