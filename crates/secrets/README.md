# secrets

Local store retrieving secrets by provider-independent logical IDs. It owns the storage format and validation that keep secrets out of scattered configuration files, not provider-specific authentication protocols or caller logging controls.

## Main entry points

- [`SecretId`] — logical ID validation
- [`SecretValue`] — secret values with redacted Debug output
- [`SecretStore`] — local storage, integrity checks, and protection limits
