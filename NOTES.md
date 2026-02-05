# Notes

## Usage/Cost Tracking - Things to Test

- **Multi-turn conversations** - Does ResultMessage accumulate usage across all turns, or just the last turn?
- **Tool calls** - Do ToolUse/ToolResult events include incremental usage data?
- **Streaming** - Are there intermediate usage updates during long responses? (AssistantMessage showed `output_tokens: 1` mid-stream)
- **Cache behavior** - When does cache_read vs cache_creation show up? Multi-turn same session? Repeated system prompts?
- **Error cases** - What does usage look like when a request fails partway through?
