package app.porchlight.shared.onboarding

/** API 30 predates the permission; 31/32 use special access; 33+ declares USE_EXACT_ALARM. */
fun exactAlarmsGranted(api: Int, capability: Boolean) = api < 31 || capability
fun needsExactAlarmSpecialPermission(api: Int) = api in 31..32
