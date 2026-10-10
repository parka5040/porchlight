package app.porchlight.shared.trust

import app.porchlight.shared.onboarding.*

import app.porchlight.shared.transport.*
import kotlin.test.*
import kotlin.time.Instant

class TrustContractTest {
    @Test fun developmentHintIsAndroidDebugOnly() {
        assertTrue(showDevelopmentHint(android = true, debugBuild = true))
        assertFalse(showDevelopmentHint(android = true, debugBuild = false))
        assertFalse(showDevelopmentHint(android = false, debugBuild = true))
    }
    @Test fun malformedPercentEscapeNeverEchoesToken() {
        val error = assertFailsWith<IllegalArgumentException> {
            EnrollUri.parse("porchlight://enroll?host=localhost&token=private%zz&ca=sha256:${"ab".repeat(32)}")
        }
        assertEquals("Invalid enrollment URI", error.message)
        assertNull(error.cause)
    }
    @Test fun exactAlarmPolicyAcrossSupportedApis() {
        for (api in 30..36) {
            assertEquals(api < 31, exactAlarmsGranted(api, false))
            assertTrue(exactAlarmsGranted(api, true))
            assertEquals(api in 31..32, needsExactAlarmSpecialPermission(api))
        }
    }

    private val ca = "ab".repeat(32)
    private val valid = "porchlight://enroll?host=lan&token=test&ca=sha256:$ca"
    @Test fun rustUriGoldenVectors() {
        for (host in listOf("porchlight.lan", "10.0.2.2:8444", "name:9444", "[fd00::1]:9000")) {
            val uri = EnrollUri.parse(valid.replace("host=lan", "host=$host"))
            assertEquals(host, uri.host)
            assertEquals("test", uri.token)
            assertFalse(uri.toString().contains("test"))
        }
        assertEquals(EnrollUri.parse(valid), EnrollUri.parse(valid.replace("enroll?", "enroll/?")))
        assertEquals(EnrollUri.parse(valid), EnrollUri.parse("$valid&extra=value&extra=other&future="))
        assertEquals(ca, EnrollUri.parse(valid.replace("sha256:$ca", "SHA256:${ca.uppercase()}")).caHex)
        val encoded = EnrollUri.parse(valid.replace("host=lan", "host=%5B%3A%3A1%5D%3A9444%26%3D%2B%23%25+%C3%A9").replace("token=test", "token=ple_a_-%26%3D%3F+%2B%C3%A9%23%25%0D%0A"))
        assertEquals("[::1]:9444&=+#% é", encoded.host)
        assertEquals("ple_a_-&=? +é#%\r\n", encoded.token)
        for (invalid in listOf(
            "this is not a URI", valid.replace("porchlight://", "https://"), valid.replace("//enroll?", "//other?"),
            valid.replace("host=lan", "host=lan&host=lan"), "$valid&host=other", "$valid&token=other", "$valid&ca=sha256:$ca",
            valid.replace("host=lan", "host="), valid.replace("token=test", "token="), valid.replace("sha256:", "sha512:"),
            valid.replace("ab", "gg"), valid.replace("//enroll?", "//user@enroll?"), valid.replace("//enroll?", "//enroll:8444?"),
            valid.replace("//enroll?", "//enroll/path?"), "$valid#fragment", valid.substringBefore("&ca="),
            "porchlight://enroll?token=test&ca=sha256:bad", "porchlight://enroll?host=lan&ca=sha256:bad"
        )) assertFailsWith<IllegalArgumentException> { EnrollUri.parse(invalid) }
    }
    @Test fun enrollmentTargets() {
        assertEquals("https://lan:8444", EnrollmentTarget.parse("lan").baseUrl)
        assertEquals("https://[fd00::1]:9000", EnrollmentTarget.parse("[fd00::1]:9000").baseUrl)
        for (bad in listOf("", "a:0", "a:65536", "a:", "::1", "[no]:1", "[:::]:1", "[1:2:3]:1", "[12345::1]:1", "a/path", "a@b", "a b", "a:xyz"))
            assertFails { EnrollmentTarget.parse(bad) }
    }
    @Test fun headerPolicy() {
        var n = 0
        val policy = HeaderPolicy(ClientInfo("0.1.0"), { "Europe/Helsinki" }, { (++n).toString() })
        val get = policy.headers("GET", "session")
        assertEquals("0.1.0", get["Porchlight-Client-Version"])
        assertEquals("Europe/Helsinki", get["Porchlight-TZ"])
        assertEquals("Bearer session", get["Authorization"])
        assertNull(get["Idempotency-Key"])
        assertNotEquals(policy.headers("POST")["Idempotency-Key"], policy.headers("POST")["Idempotency-Key"])
        val supplied = policy.headers("DELETE", idempotencyKey = "stable", version = 4, stepUp = "up")
        assertEquals("stable", supplied["Idempotency-Key"])
        assertEquals("4", supplied["If-Match"])
        assertEquals("up", supplied["Porchlight-Step-Up"])
        assertNull(supplied["Authorization"])
    }
    @Test fun problemAndStates() {
        val p = ApiProblem.decode("""{"type":"urn:porchlight:problem:upgrade-required","status":426,"title":"Upgrade","detail":null,"min_client_version":"2.0.0","current":{"version":3}}""")
        assertEquals("upgrade-required", p.type)
        assertTrue(p.extras.containsKey("current"))
        assertEquals(TrustState.UpgradeRequired("2.0.0"), stateForProblem(p))
        for (slug in listOf("session-invalid", "session-device-mismatch", "unauthenticated"))
            assertEquals(TrustState.SessionExpired, stateForProblem(ApiProblem(slug, 401, "Expired")))
        for (slug in listOf("device-revoked", "device-unknown", "device-certificate-expired"))
            assertEquals(TrustState.ReEnroll, stateForProblem(ApiProblem(slug, 401, "Revoked")))
        assertNull(stateForProblem(ApiProblem("invalid-credentials", 401, "Wrong password")))
        assertTrue(terminalRetrySuccess("logout", ApiProblem("session-invalid", 401, "")))
        assertTrue(terminalRetrySuccess("self-revoke", ApiProblem("device-revoked", 401, "")))
        assertFalse(terminalRetrySuccess("revoke", ApiProblem("device-revoked", 401, "")))
    }
    @Test fun malformedProblemDiagnosticsDoNotEchoSecrets() {
        val secret = "private_response_value"
        val error = assertFails { ApiProblem.decode("{\"token\":\"$secret\"") }
        assertFalse(error.toString().contains(secret))
        assertFalse(error.message.orEmpty().contains(secret))
    }
    @Test fun renewalAndReset() {
        val now = Instant.parse("2026-01-01T00:00:00Z")
        assertFalse(needsRenewal(now, Instant.parse("2026-01-31T00:00:00Z")))
        assertTrue(needsRenewal(now, Instant.parse("2026-01-30T23:59:59Z")))
        assertTrue(needsRenewal(now, now))
        assertEquals("ABCDEFGHJK", normalizeResetCode("abcde fghjk"))
        assertEquals("01110XXXXX", normalizeResetCode("o1il0-xxxxx"))
        for (bad in listOf("123456789", "12345678901", "12345-6789U", "12345\t67890", "é234567890"))
            assertNull(normalizeResetCode(bad))
    }
}
