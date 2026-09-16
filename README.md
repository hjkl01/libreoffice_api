# libreoffice_api

A Dockerized LibreOffice conversion service written in Rust.

## Features

- HTTP API for converting documents, spreadsheets, presentations and other LibreOffice-supported formats.
- PDF conversion by default.
- Optional target format via multipart form field `format`.
- Presentation-to-image conversion can select a specific slide with the `slide` field.
- Converted files keep the original filename and only replace the extension.
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

The downloaded filename is based on the original filename with only its extension replaced. For example, `测试文档.docx` becomes `测试文档.pdf`, and `项目报告 2026.docx` becomes `项目报告 2026.pdf`.

For example, LibreOffice-supported targets can be requested with values such as `pdf`, `docx`, `pptx`, `xlsx`, `odt`, `ods`, `odp`, `png`, `jpg`, `jpeg`, `webp`, `svg`, `txt`, `html`, etc. Availability depends on the installed LibreOffice filters.

### Convert a specific PPT/PPTX slide to an image

Use the `slide` multipart field to select a 1-based slide number when the target format is an image:

```bash
curl -X POST http://localhost:8080/convert \
  -F 'file=@example.pptx' \
  -F 'format=png' \
  -F 'slide=3' \
  -o example.png
```

The example converts only slide 3 to `example.png`. Supported image targets are `png`, `jpg`, `jpeg`, `webp`, and `svg`.

If `slide` is omitted, image conversion keeps LibreOffice's normal conversion behavior. `slide` is only valid for image output formats.

## Configuration

- `PORT`: HTTP port, default `8080`.
- `MAX_UPLOAD_SIZE`: maximum upload size in bytes, default `104857600` (100 MiB).
- `LO_MAX_CONCURRENCY`: maximum simultaneous LibreOffice conversions, default `2`.
- `CONVERSION_TIMEOUT_SECS`: per-conversion timeout, default `300`.

The service does not invoke a shell. Uploaded filenames are never passed through a shell command.
