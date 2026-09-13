# Fixtures

Each `<service>-<version>/` directory holds answers **recorded** from a running
instance; its `SOURCE.md` says when and how.

`constructed-validation-error.json` is **not** recorded. It follows the
FluentValidation shape Radarr and Sonarr return for a rejected write
(`propertyName`, `errorMessage`, …). Provoking a real one on a live instance
was not worth the risk of a partial write.

Inside a service directory, a file named `constructed-…` is built by hand as
well; that directory's `SOURCE.md` says why and what it was built from.
