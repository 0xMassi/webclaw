# HTML to Markdown for RAG

Turn web pages into clean markdown or compact LLM text before chunking, embedding, or passing the page to an agent.

## CLI

```bash
# Clean markdown with headings, links, and readable structure.
webclaw https://docs.anthropic.com --format markdown > page.md

# Token-optimized output for direct LLM context.
webclaw https://docs.anthropic.com --format llm > page.txt

# Keep the main article content and remove common navigation/footer noise.
webclaw https://docs.anthropic.com \
  --only-main-content \
  --format markdown \
  > page.md
```

## Batch a URL List

Create `urls.txt`:

```text
https://docs.anthropic.com/
https://docs.anthropic.com/en/docs/claude-code
https://docs.anthropic.com/en/api/messages
```

Run:

```bash
webclaw --urls-file urls.txt --format llm > corpus.txt
```

## Hosted API

```bash
curl https://api.webclaw.io/v1/scrape \
  -H "Authorization: Bearer $WEBCLAW_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "url": "https://docs.anthropic.com",
    "formats": ["markdown", "llm"],
    "only_main_content": true
  }'
```

Use `markdown` when humans may inspect the output. Use `llm` when the next step is chunking, embedding, summarization, or prompt context.
