# libreoffice_api

A Dockerized LibreOffice conversion service written in Rust.

## Features

- HTTP API for converting documents, spreadsheets, presentations and other LibreOffice-supported formats.
- PDF conversion by default.
- Optional target format via multipart form field `format`.
- Concurrent conversions with a configurable limit.
- Every conversion uses an isolated LibreOffice user profile and temporary directory, avoiding profile/IPC collisions between instances.
- Docker image includes LibreOffice and a useful set of Latin, CJK and common Microsoft-compatible fonts.

## Run

```bash
docker compose up --build
```

The service listens on `0.0.0.0:8080`.

## API

### Health

```bash
curl http://localhost:8080/health
```

### Convert to PDF

```bash
curl -X POST http://localhost:8080/convert \
  -F 'file=@example.pptx' \
  -o example.pdf
```

### Convert to another format

```bash
curl -X POST http://localhost:8080/convert \
  -F 'file=@example.docx' \
  -F 'format=pdf' \
  -o example.pdf
```

For example, LibreOffice-supported targets can be requested with values such as `pdf`, `docx`, `pptx`, `xlsx`, `odt`, `ods`, `odp`, `txt`, `html`, etc. Availability depends on the installed LibreOffice filters.

## Configuration

- `PORT`: HTTP port, default `8080`.
- `MAX_UPLOAD_SIZE`: maximum upload size in bytes, default `104857600` (100 MiB).
- `LO_MAX_CONCURRENCY`: maximum simultaneous LibreOffice conversions, default `2`.
- `CONVERSION_TIMEOUT_SECS`: per-conversion timeout, default `300`.

The service does not invoke a shell. Uploaded filenames are never passed through a shell command.
