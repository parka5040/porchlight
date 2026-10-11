package app.porchlight.shared.data

import app.cash.sqldelight.driver.jdbc.sqlite.JdbcSqliteDriver
import app.porchlight.shared.db.PorchlightDb
import kotlin.time.Instant

actual fun memoryCache(attachmentStore: AttachmentStore, now: () -> Instant): ReadCache {
    val driver = JdbcSqliteDriver(JdbcSqliteDriver.IN_MEMORY)
    PorchlightDb.Schema.create(driver)
    return ReadCache({ driver }, now, null, attachmentStore)
}
