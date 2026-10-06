package com.cliwalker.tui

// Block-font "CLI WALKER", coloured at runtime with a nebula gradient. The art is 77
// columns wide, centred under the 80-column dashboard box below it — it must not grow past
// 80: a wider logo wraps and the last letters are lost. Letters sit flush against each
// other so the gap between the two words stays wide enough to read as a space.
private val LOGO_ART = listOf(
    "    ██████╗██╗     ██╗     ██╗    ██╗ █████╗ ██╗     ██╗  ██╗███████╗██████╗",
    "   ██╔════╝██║     ██║     ██║    ██║██╔══██╗██║     ██║ ██╔╝██╔════╝██╔══██╗",
    "   ██║     ██║     ██║     ██║ █╗ ██║███████║██║     █████╔╝ █████╗  ██████╔╝",
    "   ██║     ██║     ██║     ██║███╗██║██╔══██║██║     ██╔═██╗ ██╔══╝  ██╔══██╗",
    "   ╚██████╗███████╗██║     ╚███╔███╔╝██║  ██║███████╗██║  ██╗███████╗██║  ██║",
    "    ╚═════╝╚══════╝╚═╝      ╚══╝╚══╝ ╚═╝  ╚═╝╚══════╝╚═╝  ╚═╝╚══════╝╚═╝  ╚═╝",
)

private const val LOGO_WIDTH = 80

// Nebula: purple on the left fading to pink on the right.
private val GRADIENT_START = Triple(101, 78, 163)
private val GRADIENT_END = Triple(234, 175, 200)

private fun gradientAt(column: Int): String {
    val t = column.toDouble() / (LOGO_WIDTH - 1)
    fun mix(a: Int, b: Int) = (a + (b - a) * t).toInt()
    val r = mix(GRADIENT_START.first, GRADIENT_END.first)
    val g = mix(GRADIENT_START.second, GRADIENT_END.second)
    val b = mix(GRADIENT_START.third, GRADIENT_END.third)
    return "\u001B[38;2;$r;$g;${b}m"
}

val LOGO: String = LOGO_ART.joinToString("\n") { row ->
    buildString {
        row.forEachIndexed { column, char ->
            // Spaces carry no ink, so skip the escape for them and keep the line short.
            if (char != ' ') append(gradientAt(column))
            append(char)
        }
        append("\u001B[39m")
    }
} + "\n\n"

fun printLogo() = print(LOGO)

// Clears the screen and prints the logo above where the dashboard is drawn.
fun printHeader() {
    print("\u001B[2J\u001B[H\n")
    printLogo()
}
