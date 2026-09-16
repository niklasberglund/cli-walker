plugins {
    kotlin("multiplatform") version "2.2.10"
    kotlin("plugin.serialization") version "2.2.10"
}

repositories {
    mavenCentral()
}

kotlin {
    val hostOs = System.getProperty("os.name")
    val hostArch = System.getProperty("os.arch")

    val macTarget = when {
        hostOs == "Mac OS X" && hostArch == "aarch64" -> macosArm64()
        hostOs == "Mac OS X" -> macosX64()
        else -> throw GradleException("Host OS '$hostOs' is not supported.")
    }

    macTarget.apply {
        binaries {
            executable {
                entryPoint = "com.cliwalker.main"
                baseName = "cli-walker"
            }
        }
    }

    sourceSets {
        commonMain.dependencies {
            implementation("com.juul.kable:kable-core:0.40.0")
            implementation("com.github.ajalt.mordant:mordant:3.0.2")
            implementation("com.github.ajalt.mordant:mordant-coroutines:3.0.2")
            implementation("com.github.ajalt.clikt:clikt:5.0.3")
            implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.10.1")
            implementation("org.jetbrains.kotlinx:kotlinx-datetime:0.6.2")
            implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.7.3")
            implementation("io.ktor:ktor-client-core:3.1.3")
            implementation("io.ktor:ktor-client-darwin:3.1.3")
        }
        commonTest.dependencies {
            implementation(kotlin("test"))
        }
    }
}
