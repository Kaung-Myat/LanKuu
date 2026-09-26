plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

dependencies {
    implementation("io.github.webrtc-sdk:android:150.7871.01")
}

android {
    namespace = "dev.lankuu.app"
    compileSdk = 36

    defaultConfig {
        applicationId = "dev.lankuu.app"
        minSdk = 29
        targetSdk = 36
        versionCode = 4
        versionName = "0.2.1"
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}
