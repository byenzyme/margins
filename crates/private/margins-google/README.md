# Margins Google OAuth resource

Official builds inject `MARGINS_GOOGLE_OAUTH_CLIENT_JSON` from a CI secret at
compile time. Source builds can set that variable at runtime or provide a file
with `MARGINS_GOOGLE_OAUTH_CLIENT_FILE`. The value must be a Google OAuth
**Desktop app** client in Google's downloaded JSON shape:

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

No client JSON is tracked in this repository. Official CI provides the JSON
only to the CLI build step; the runtime file option keeps local credentials out
of source.
