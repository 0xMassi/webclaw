# Deploying webclaw-api on Dokploy

`webclaw-api` is a lightweight, high-performance REST API wrapper for the webclaw extraction engine. Built with Rust and Axum, it's designed for low-resource environments and seamless integration with external systems.

## Prerequisites

- A [Dokploy](https://dokploy.com/) instance running on your server.
- The webclaw repository connected to your Dokploy (via GitHub or local upload).

## Deployment Steps

1.  **Create a New Application**: 
    - In Dokploy, create a new application pointing to your webclaw repository.
    - Select **Docker** as the deployment method.

2.  **Environment Configuration**:
    Navigate to **Environment Settings** and add the following variables:
    - `PORT`: `3000` (The port the API will listen on).
    - `API_KEY`: `your_secure_random_token` (Used for Bearer authentication).
    
    *Optional (for LLM features):*
    - `OPENAI_API_KEY` or `ANTHROPIC_API_KEY`.

3.  **Command Configuration**:
    In the deployment settings, ensure the **Command** (Entrypoint) is set explicitly to:
    ```bash
    webclaw-api
    ```
    This ensures that Dokploy runs the REST API server instead of the CLI or MCP server.

4.  **Network Setup**:
    - Mapping: Map container port `3000` to your desired host port or use a domain with SSL.

## Usage

Once deployed, you can interact with the API using any HTTP client.

### Authentication
All requests must include the `Authorization` header if `API_KEY` is set:
`Authorization: Bearer <your_secure_random_token>`

### Endpoint: `POST /api/scrape`

**Request Body:**
```json
{
  "url": "https://example.com"
}
```

**Example via `curl`:**
```bash
curl -X POST https://your-webclaw-api.com/api/scrape \
     -H "Authorization: Bearer your_secure_random_token" \
     -H "Content-Type: application/json" \
     -d '{"url": "https://cnpja.com/office/07526557011659"}'
```

### Response Format
The API returns a full `ExtractionResult` JSON object, including `metadata`, `content`, and the improved `structured_data` (Data Islands).

## Performance & Resources
- **Idle Memory**: ~20MB - 30MB RAM.
- **CPU**: Near zero when idle; efficient single-threaded or multi-threaded processing on request.
