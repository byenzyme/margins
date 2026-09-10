# Margins Google OAuth resource

The official private composition embeds `resources/google-oauth-client.json` at
compile time. The owner replaces the documented placeholder before producing an
official build. It must be a Google OAuth **Desktop app** client in Google's
downloaded JSON shape:

```json
{
  "installed": {
    "client_id": "…apps.googleusercontent.com",
    "project_id": "…",
    "auth_uri": "https://accounts.google.com/o/oauth2/auth",
    "token_uri": "https://oauth2.googleapis.com/token",
    "auth_provider_x509_cert_url": "https://www.googleapis.com/oauth2/v1/certs",
    "client_secret": "…",
    "redirect_uris": ["http://localhost"]
  }
}
```

This directory is private-composition-only and must never be added to
`open-source-boundary.json`.
