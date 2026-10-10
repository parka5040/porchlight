package app.porchlight.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import app.porchlight.shared.testing.ShellFixture

/** Debug-only entry point; deterministic composables and transport live in shared test support. */
class ShellFixtureActivity : ComponentActivity() {
    val controllerIdentity get() = app.porchlight.shared.testing.fixtureControllerIdentity()
    val resumeRoute get() = app.porchlight.shared.testing.fixtureResumeRoute()
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent { ShellFixture() }
    }
}
