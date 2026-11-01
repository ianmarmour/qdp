package io.github.ianmarmour.qdp

import android.content.Context
import android.text.InputType
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager

/**
 * Takes the on-screen keyboard's typing: committed text goes to the host as
 * text, and Enter, Backspace and other keys as key presses through [onKey].
 */
class TypingView(context: Context, private val onKey: (KeyEvent) -> Boolean) : View(context) {
    private val inputMethods = context.getSystemService(InputMethodManager::class.java)

    init {
        isFocusable = true
        isFocusableInTouchMode = true
    }

    /** Shows the on-screen keyboard. */
    fun show() {
        requestFocus()
        inputMethods.showSoftInput(this, 0)
    }

    /** Hides the on-screen keyboard. */
    fun hide() {
        inputMethods.hideSoftInputFromWindow(windowToken, 0)
    }

    override fun onCheckIsTextEditor() = true

    override fun onCreateInputConnection(outAttrs: EditorInfo): InputConnection {
        // No suggestions or composing: each character is sent as typed.
        outAttrs.inputType = InputType.TYPE_CLASS_TEXT or
            InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD or
            InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
        outAttrs.imeOptions = EditorInfo.IME_FLAG_NO_FULLSCREEN or
            EditorInfo.IME_FLAG_NO_EXTRACT_UI or
            EditorInfo.IME_ACTION_NONE
        return object : BaseInputConnection(this, false) {
            override fun commitText(text: CharSequence, newCursorPosition: Int): Boolean {
                for (line in text.split('\n').withIndex()) {
                    if (line.index > 0) {
                        tap(KeyEvent.KEYCODE_ENTER)
                    }
                    if (line.value.isNotEmpty()) {
                        Native.text(line.value)
                    }
                }
                return true
            }

            override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
                repeat(beforeLength) { tap(KeyEvent.KEYCODE_DEL) }
                repeat(afterLength) { tap(KeyEvent.KEYCODE_FORWARD_DEL) }
                return true
            }

            override fun sendKeyEvent(event: KeyEvent): Boolean = onKey(event)
        }
    }

    /** Presses and releases the key [keyCode]. */
    private fun tap(keyCode: Int) {
        Native.key(keyCode, true)
        Native.key(keyCode, false)
    }
}
