plugins {
    kotlin("multiplatform") version "2.2.10"
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

    val targetName = if (hostArch == "aarch64") "macosArm64" else "macosX64"

    tasks.register<Exec>("run") {
        dependsOn("${targetName}Binaries")
        group = "application"
        description = "Build and run CLI-Walker (non-interactive)"
        executable = layout.buildDirectory
            .file("bin/$targetName/debugExecutable/cli-walker.kexe")
            .get().asFile.absolutePath
        standardInput = System.`in`
        standardOutput = System.out
        errorOutput = System.err
    }



    sourceSets {
        commonMain.dependencies {
            implementation("com.juul.kable:kable-core:0.40.0")
            implementation("com.github.ajalt.mordant:mordant:3.0.2")
            implementation("com.github.ajalt.mordant:mordant-coroutines:3.0.2")
            implementation("com.github.ajalt.clikt:clikt:5.0.3")
            implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.10.1")
            implementation("org.jetbrains.kotlinx:kotlinx-datetime:0.6.2")
        }
    }
}
