# feature-storage

Manages the SQLite lifecycle for Features within a trusted Host.
Does not own Feature domain schemas or authentication, or provide SQL access to Workers or models.

## Main entry points

- [`FeatureStorage`], [`ScopedFeatureStorage`]: registration, scope isolation, storage, and shutdown
- [`FeatureDatabase`]: connections and transactions
- [`FeatureMigration`]: Feature-owned schema changes
- [`configure_connection`]: shared SQLite connection policy
