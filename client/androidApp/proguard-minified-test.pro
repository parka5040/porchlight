# Minified smoke ONLY. AGP removes app-provided dependencies from the test APK, but app R8
# cannot see their callers in AndroidJUnitRunner/monitor/core/ext-junit or the smoke class.
# Keep that cross-APK ABI, not whole libraries or application/probe code. Member bodies may
# optimize; descriptor classes retain names. Empty facade rules preserve inherited JVM lookup.
# Audited against runner/core 1.7.0, monitor 1.8.0, ext-junit 1.3.0 and the compiled smoke.
# deploy/tests/audit-apk-fix1.py checks actual dex linkage after both APKs are shrunk.

# Runner/monitor tracing, ActivityScenario lifecycle and its callback futures.
-keep,allowoptimization,includedescriptorclasses class androidx.tracing.Trace {
    static void beginSection(java.lang.String);
    static void endSection();
    static void forceEnableAppTracing();
}
-keep,allowoptimization,includedescriptorclasses class androidx.concurrent.futures.CallbackToFutureAdapter {
    static com.google.common.util.concurrent.ListenableFuture getFuture(androidx.concurrent.futures.CallbackToFutureAdapter$Resolver);
}
-keep,allowoptimization,includedescriptorclasses class androidx.concurrent.futures.CallbackToFutureAdapter$Completer {
    boolean set(java.lang.Object);
    boolean setException(java.lang.Throwable);
}
-keep,allowoptimization,includedescriptorclasses class androidx.concurrent.futures.CallbackToFutureAdapter$Resolver {
    java.lang.Object attachCompleter(androidx.concurrent.futures.CallbackToFutureAdapter$Completer);
}
-keep,allowoptimization,includedescriptorclasses class androidx.lifecycle.Lifecycle$State {
    static androidx.lifecycle.Lifecycle$State CREATED;
    static androidx.lifecycle.Lifecycle$State DESTROYED;
    static androidx.lifecycle.Lifecycle$State RESUMED;
    static androidx.lifecycle.Lifecycle$State STARTED;
    static androidx.lifecycle.Lifecycle$State[] values();
}
-keep,allowoptimization,includedescriptorclasses class com.google.common.util.concurrent.ListenableFuture {
    void addListener(java.lang.Runnable,java.util.concurrent.Executor);
}

# Kotlin ABI used by AndroidX test core (including its coroutine subclasses) and our smoke.
-keep,allowoptimization,includedescriptorclasses class kotlin.Lazy {
    java.lang.Object getValue();
}
-keep,allowoptimization,includedescriptorclasses class kotlin.LazyKt {}
-keep,allowoptimization,includedescriptorclasses class kotlin.LazyKt__LazyJVMKt {
    static kotlin.Lazy lazy(kotlin.jvm.functions.Function0);
}
-keep,allowoptimization,includedescriptorclasses class kotlin.Result {
    static kotlin.Result$Companion Companion;
    static java.lang.Object constructor-impl(java.lang.Object);
}
-keep,allowoptimization,includedescriptorclasses class kotlin.ResultKt {
    static java.lang.Object createFailure(java.lang.Throwable);
    static void throwOnFailure(java.lang.Object);
}
-keep,allowoptimization,includedescriptorclasses class kotlin.Unit {
    static kotlin.Unit INSTANCE;
}
-keep,allowoptimization,includedescriptorclasses class kotlin.collections.IntIterator {
    int nextInt();
}
-keep,allowoptimization,includedescriptorclasses class kotlin.coroutines.Continuation {
    void resumeWith(java.lang.Object);
}
-keep,allowoptimization,includedescriptorclasses class kotlin.coroutines.intrinsics.IntrinsicsKt {}
-keep,allowoptimization,includedescriptorclasses class kotlin.coroutines.intrinsics.IntrinsicsKt__IntrinsicsJvmKt {
    static kotlin.coroutines.Continuation intercepted(kotlin.coroutines.Continuation);
}
-keep,allowoptimization,includedescriptorclasses class kotlin.coroutines.intrinsics.IntrinsicsKt__IntrinsicsKt {
    static java.lang.Object getCOROUTINE_SUSPENDED();
}
-keep,allowoptimization,includedescriptorclasses class kotlin.coroutines.jvm.internal.BaseContinuationImpl {
    kotlin.coroutines.Continuation create(java.lang.Object,kotlin.coroutines.Continuation);
    java.lang.Object invokeSuspend(java.lang.Object);
}
-keep,allowoptimization,includedescriptorclasses class kotlin.coroutines.jvm.internal.ContinuationImpl {
    <init>(kotlin.coroutines.Continuation);
}
-keep,allowoptimization,includedescriptorclasses class kotlin.coroutines.jvm.internal.DebugProbesKt {
    static void probeCoroutineSuspended(kotlin.coroutines.Continuation);
}
-keep,allowoptimization,includedescriptorclasses class kotlin.coroutines.jvm.internal.SuspendLambda {
    <init>(int,kotlin.coroutines.Continuation);
}
-keep,allowoptimization,includedescriptorclasses class kotlin.io.CloseableKt {
    static void closeFinally(java.io.Closeable,java.lang.Throwable);
}
-keep,allowoptimization,includedescriptorclasses class kotlin.jvm.functions.Function0 {
    java.lang.Object invoke();
}
-keep,allowoptimization,includedescriptorclasses class kotlin.jvm.functions.Function1 {
    java.lang.Object invoke(java.lang.Object);
}
-keep,allowoptimization,includedescriptorclasses class kotlin.jvm.functions.Function2 {
    java.lang.Object invoke(java.lang.Object,java.lang.Object);
}
-keep,allowoptimization,includedescriptorclasses class kotlin.jvm.internal.Intrinsics {
    static boolean areEqual(java.lang.Object,java.lang.Object);
    static void checkNotNull(java.lang.Object);
    static void checkNotNullExpressionValue(java.lang.Object,java.lang.String);
    static void checkNotNullParameter(java.lang.Object,java.lang.String);
}
-keep,allowoptimization,includedescriptorclasses class kotlin.jvm.internal.Lambda {
    <init>(int);
}
-keep,allowoptimization,includedescriptorclasses class kotlin.jvm.internal.Ref$ObjectRef {
    <init>();
    java.lang.Object element;
}
-keep,allowoptimization,includedescriptorclasses class kotlin.jvm.internal.StringCompanionObject {
    static kotlin.jvm.internal.StringCompanionObject INSTANCE;
}
-keep,allowoptimization,includedescriptorclasses class kotlin.ranges.RangesKt {}
-keep,allowoptimization,includedescriptorclasses class kotlin.ranges.RangesKt___RangesKt {
    static kotlin.ranges.IntRange until(int,int);
}
-keep,allowoptimization,includedescriptorclasses class kotlin.time.Duration {
    static kotlin.time.Duration$Companion Companion;
}
-keep,allowoptimization,includedescriptorclasses class kotlin.time.DurationKt {
    static long toDuration(int,kotlin.time.DurationUnit);
}
-keep,allowoptimization,includedescriptorclasses class kotlin.time.DurationUnit {
    static kotlin.time.DurationUnit SECONDS;
}
-keep,allowoptimization,includedescriptorclasses class kotlinx.coroutines.BuildersKt {
    static java.lang.Object runBlocking(kotlin.coroutines.CoroutineContext,kotlin.jvm.functions.Function2);
}
-keep,allowoptimization,includedescriptorclasses class kotlinx.coroutines.CancellableContinuation {
    void resume(java.lang.Object,kotlin.jvm.functions.Function1);
}
-keep,allowoptimization,includedescriptorclasses class kotlinx.coroutines.CancellableContinuationImpl {
    <init>(kotlin.coroutines.Continuation,int);
    java.lang.Object getResult();
    void initCancellability();
}
-keep,allowoptimization,includedescriptorclasses class kotlinx.coroutines.Dispatchers {
    static kotlinx.coroutines.MainCoroutineDispatcher getMain();
}
-keep,allowoptimization,includedescriptorclasses class kotlinx.coroutines.ExecutorsKt {
    static kotlinx.coroutines.CoroutineDispatcher from(java.util.concurrent.Executor);
}
-keep,allowoptimization,includedescriptorclasses class kotlinx.coroutines.TimeoutKt {
    static java.lang.Object withTimeout-KLykuaI(long,kotlin.jvm.functions.Function2,kotlin.coroutines.Continuation);
}
