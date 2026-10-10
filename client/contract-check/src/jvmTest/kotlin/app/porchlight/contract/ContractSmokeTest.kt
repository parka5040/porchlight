package app.porchlight.contract

import app.porchlight.contract.models.*
import kotlinx.serialization.json.*
import kotlin.test.*
import kotlin.time.Instant

class ContractSmokeTest {
    private val json = Json { ignoreUnknownKeys = false }
    private inline fun <reified T> fixture(name: String): T = json.decodeFromString(
        checkNotNull(javaClass.getResource("/fixtures/$name.json")).readText())

    @Test fun currentIdentityFixtureDecodesUsingGeneratedModel() {
        val me = fixture<MeResponse>("me")
        assertEquals("Alex", me.displayName)
        assertEquals("android", me.device.platform)
        assertEquals(Instant.parse("2027-04-01T00:00:00Z"), me.sessionExpiresAt)
    }
    @Test fun ownDevicesFixtureDecodesBothCollectionShapesAndNulls() {
        val devices = fixture<DevicesResponse>("devices")
        assertEquals(devices.devices, devices.items)
        assertEquals(2, devices.devices.size)
        assertTrue(devices.devices.first().current)
        assertNull(devices.devices.first().revokedAt)
        assertEquals(Instant.parse("2026-10-04T12:00:00Z"), devices.devices.last().revokedAt)
        assertNull(devices.nextCursor)
        assertTrue(devices.devices.all { it.username == null })
    }
    @Test fun freeFormSchemaMappingsHaveAnExplicitAllowlist() {
        val spec = Json.parseToJsonElement(java.io.File("../../docs/openapi.json").readText()).jsonObject
        val paths = mutableSetOf<String>()
        fun scan(schema: JsonObject, path: String) {
            val types = schema["type"]
            val objectType = types == JsonPrimitive("object") || (types is JsonArray && JsonPrimitive("object") in types)
            if (schema.isEmpty() || (objectType && "properties" !in schema && schema["additionalProperties"] != JsonPrimitive(false))) paths += path
            schema["properties"]?.jsonObject?.forEach { (name, value) -> scan(value.jsonObject, "$path/properties/$name") }
            schema["items"]?.let { scan(it.jsonObject, "$path/items") }
            listOf("oneOf", "anyOf", "allOf").forEach { key ->
                schema[key]?.jsonArray?.forEachIndexed { i, value -> scan(value.jsonObject, "$path/$key/$i") }
            }
        }
        spec.getValue("components").jsonObject.getValue("schemas").jsonObject.forEach { (name, schema) -> scan(schema.jsonObject, name) }
        // Data-only bulk export is intentionally a versioned raw table inventory,
        // not an interactive client domain model. O2b saves the member JSON/ICS.
        assertEquals(setOf("ProblemBody/properties/current", "MemberExport/properties/tables"), paths,
            "New Object/AnyType mappings require a typed schema or an explicitly reviewed exception")
    }
    @Test fun problemFixtureKeepsUpgradeContract() {
        val problem = fixture<ProblemBody>("problem")
        assertEquals(426, problem.status)
        assertEquals("upgrade-required", problem.type.substringAfterLast(':'))
        assertEquals("0.2.0", problem.minClientVersion)
    }
}
