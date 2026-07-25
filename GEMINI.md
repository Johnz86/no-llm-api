# Project Overview

This project is a Rust-based backend service that simulates the OpenAI chat completions API. It provides a testing backend for GUI chat applications without using any actual large language models (LLMs). The conversation responses are served from Parquet files, and the response speed (tokens per second) is configurable.

The API is defined in `openapi.yaml` and follows the OpenAI REST API specification.

## Technologies

*   **Language:** Rust
*   **Web Framework:** Axum
*   **Async Runtime:** Tokio
*   **Data Format:** Parquet, JSON
*   **Configuration:** Dotenv
*   **Logging:** Tracing

## Architecture

The application is structured into several modules:

*   `main.rs`: The entry point of the application. It initializes the services, router, and starts the server.
*   `http`: Contains the web-related logic, including the Axum router and route handlers.
*   `service`: Implements the core business logic for chat completions.
*   `store`: Handles the data storage and retrieval from Parquet files.
*   `model`: Defines the data structures for API requests and responses.
*   `dataset`: Manages the conversation scripts from Parquet files.
*   `config`: Handles the application settings.

# Building and Running

## Prerequisites

*   Rust toolchain

## Building

To build the project, run the following command:

```bash
cargo build
```

## Running

To run the application, use the following command:

```bash
cargo run
```

The server will start on the address specified in the `.env` file or the default configuration.

## Testing

To run the tests, use the following command:

```bash
cargo test
```

# Development Conventions

*   **Code Style:** The project follows the standard Rust formatting guidelines. Use `cargo fmt` to format the code.
*   **API Definition:** The API is defined in the `openapi.yaml` file. Any changes to the API should be reflected in this file.
*   **Configuration:** Application configuration is managed through a `.env` file. A `.env.example` file should be provided to list the required environment variables.
*   **Dependencies:** Dependencies are managed with Cargo. Use `cargo add` to add new dependencies.
*   **Logging:** The application uses the `tracing` library for logging.
