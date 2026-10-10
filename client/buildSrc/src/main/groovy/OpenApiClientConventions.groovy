/**
 * One OpenAPI Kotlin generator configuration for C2a's :contract-check drift gate and the app's :shared client, so
 * the two cannot drift. :shared adds only the reviewed free-form mappings in SHARED_SCHEMA_MAPPINGS.
 */
class OpenApiClientConventions {
    public static final Map<String, String> CONFIG_OPTIONS = [dateLibrary: 'kotlinx-datetime']
    /** Area-owned zero-field request types map to an object, as for the M2 shapes (S6a adds ReadRequest). */
    public static final Map<String, String> SCHEMA_MAPPINGS = [
        Empty: 'JsonObject', Logout: 'JsonObject', EmptyRequest: 'JsonObject', ReadRequest: 'JsonObject',
    ]
    public static final Map<String, String> TYPE_MAPPINGS = [object: 'JsonElement', AnyType: 'JsonElement']
    public static final Map<String, String> IMPORT_MAPPINGS = [
        JsonElement: 'kotlinx.serialization.json.JsonElement',
        JsonObject : 'kotlinx.serialization.json.JsonObject',
    ]
    /**
     * The complete reviewed set of extra :shared mappings (anything else needs review):
     * - PatchRequest, OccurrencePatch: PATCH presence must distinguish an omitted field from an explicit null, and
     *   wall-clock fields stay strings (ItemEdit builds the document);
     * - SearchRead: a oneOf (item | list hit) that the generator flattens into one class requiring both shapes'
     *   fields, which cannot decode either; SearchRepository dispatches on entity_type instead.
     */
    public static final Map<String, String> SHARED_SCHEMA_MAPPINGS = SCHEMA_MAPPINGS + [
        PatchRequest: 'JsonObject', OccurrencePatch: 'JsonObject', SearchRead: 'JsonObject',
    ]
}
