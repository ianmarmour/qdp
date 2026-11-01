package io.github.ianmarmour.qdp

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.text.InputType
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.view.ViewGroup.LayoutParams.MATCH_PARENT
import android.view.ViewGroup.LayoutParams.WRAP_CONTENT
import android.view.WindowInsets
import android.widget.ArrayAdapter
import android.widget.Button
import android.widget.CheckBox
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.Spinner
import android.widget.TextView
import android.widget.Toast

/** The connection screen: the host to stream from and how. */
class MainActivity : Activity() {
    private lateinit var host: EditText
    private lateinit var pin: EditText
    private lateinit var token: EditText
    private lateinit var resolution: Spinner
    private lateinit var decoder: Spinner
    private lateinit var touchMode: Spinner
    private lateinit var showStats: CheckBox
    private lateinit var microphone: CheckBox

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Native.init(applicationContext)
        takeLink(intent)
        val settings = Settings.load(this)
        val form = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            val margin = dp(24)
            setPadding(margin, margin, margin, margin)
        }
        form.addView(TextView(this).apply {
            text = getString(R.string.app_name)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 28f)
        })
        host = form.field("Host", "192.168.1.20 or desktop.local:4433", settings.host)
        pin = form.field("Host key pin", "64 hexadecimal digits from `qdp pin`", settings.pin)
        token = form.field("Token", "The token the host accepts", settings.token).apply {
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD
        }
        resolution = form.choice("Resolution", Resolution.entries.map { it.label }, settings.resolution.ordinal)
        decoder = form.choice("Decoder", Decoder.entries.map { it.label }, settings.decoder.ordinal)
        touchMode = form.choice("Touch", TouchMode.entries.map { it.label }, settings.touchMode.ordinal)
        showStats = CheckBox(this).apply {
            text = "Show statistics"
            isChecked = settings.showStats
        }
        form.addView(showStats)
        microphone = CheckBox(this).apply {
            text = "Send the microphone"
            isChecked = settings.microphone
        }
        form.addView(microphone)
        form.addView(Button(this).apply {
            text = "Connect"
            setOnClickListener { connect() }
        }, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT).apply { topMargin = dp(16) })
        val scroll = ScrollView(this).apply {
            addView(form)
            // The form stays clear of the system bars and the keyboard.
            setOnApplyWindowInsetsListener { view, insets ->
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    val bars = insets.getInsets(WindowInsets.Type.systemBars() or WindowInsets.Type.ime())
                    view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
                } else {
                    @Suppress("DEPRECATION")
                    view.setPadding(
                        insets.systemWindowInsetLeft,
                        insets.systemWindowInsetTop,
                        insets.systemWindowInsetRight,
                        insets.systemWindowInsetBottom,
                    )
                }
                insets
            }
        }
        setContentView(scroll)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        if (takeLink(intent)) {
            recreate()
        }
    }

    /** Keeps the settings a connection link in [intent] gives, returning whether it had one. */
    private fun takeLink(intent: Intent?): Boolean {
        val link = intent?.data ?: return false
        val settings = ConnectionLink.apply(link, Settings.load(this)) ?: return false
        settings.save(this)
        // The link is used once, not again when the screen is recreated.
        intent.data = null
        Toast.makeText(this, "Filled in from the link", Toast.LENGTH_SHORT).show()
        return true
    }

    /** Returns `value` density-independent pixels in pixels. */
    private fun dp(value: Int): Int =
        TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_DIP, value.toFloat(), resources.displayMetrics).toInt()

    /** Adds a labelled text field showing [value]. */
    private fun LinearLayout.field(label: String, hint: String, value: String): EditText {
        addView(label(label))
        val field = EditText(context).apply {
            this.hint = hint
            setText(value)
            isSingleLine = true
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
        }
        addView(field, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT))
        return field
    }

    /** Adds a labelled choice of [options], with [selected] chosen. */
    private fun LinearLayout.choice(label: String, options: List<String>, selected: Int): Spinner {
        addView(label(label))
        val spinner = Spinner(context).apply {
            adapter = ArrayAdapter(context, android.R.layout.simple_spinner_dropdown_item, options)
            setSelection(selected)
        }
        addView(spinner, LinearLayout.LayoutParams(MATCH_PARENT, WRAP_CONTENT))
        return spinner
    }

    /** Returns a field's label. */
    private fun label(text: String): View = TextView(this).apply {
        this.text = text
        gravity = Gravity.START
        setPadding(0, dp(16), 0, 0)
    }

    /** Checks the form, keeps it, and starts streaming. */
    private fun connect() {
        val settings = Settings(
            host = host.text.toString().trim(),
            pin = pin.text.toString().trim(),
            token = token.text.toString(),
            resolution = Resolution.entries[resolution.selectedItemPosition],
            decoder = Decoder.entries[decoder.selectedItemPosition],
            touchMode = TouchMode.entries[touchMode.selectedItemPosition],
            showStats = showStats.isChecked,
            microphone = microphone.isChecked,
        )
        when {
            settings.host.isEmpty() -> host.error = "Enter the host's address"
            !Native.isPin(settings.pin) -> pin.error = "A pin is 64 hexadecimal digits, as `qdp pin` prints"
            settings.token.isEmpty() -> token.error = "Enter the token the host was started with"
            else -> {
                settings.save(this)
                val granted = checkSelfPermission(Manifest.permission.RECORD_AUDIO) ==
                    PackageManager.PERMISSION_GRANTED
                if (settings.microphone && !granted) {
                    requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), MICROPHONE_REQUEST)
                } else {
                    stream()
                }
            }
        }
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        // Without the permission the session streams without the microphone.
        if (requestCode == MICROPHONE_REQUEST) {
            stream()
        }
    }

    /** Starts the stream with the kept settings. */
    private fun stream() {
        startActivity(Intent(this, StreamActivity::class.java))
    }

    private companion object {
        const val MICROPHONE_REQUEST = 1
    }
}
