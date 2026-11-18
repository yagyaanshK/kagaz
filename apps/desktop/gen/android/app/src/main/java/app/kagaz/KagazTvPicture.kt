package app.kagaz

import android.graphics.SurfaceTexture
import android.opengl.EGL14
import android.opengl.EGLConfig
import android.opengl.EGLContext
import android.opengl.EGLDisplay
import android.opengl.EGLExt
import android.opengl.EGLSurface
import android.opengl.GLES11Ext
import android.opengl.GLES20
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import android.view.Surface
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.nio.FloatBuffer
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** Between the phone's screen and the encoder, so the picture can be
 *  hidden: the screen's pictures land in a texture and are drawn onto the
 *  encoder's surface, as they are or, while hidden, as through frosted
 *  glass.
 *
 *  The frosting cannot be undone, because the detail is not scrambled
 *  but thrown away before anything leaves the phone: each picture is
 *  averaged down to about 30 by 17 patches (each the mean of some four
 *  thousand pixels), each patch's colour is rounded to 64 steps, and
 *  only those five hundred colours are spread out again, smoothly, under
 *  a dim tint. Broad colour and large movement show; words, faces and
 *  the shapes of things do not exist in what is sent.
 *
 *  Hiding takes effect with the very next picture, and at once on a still
 *  screen (the last picture is drawn again, blurred). */
class KagazTvPicture(private val out: Surface, private val width: Int, private val height: Int, hidden: Boolean) {
    @Volatile var hidden = hidden
        set(value) {
            field = value
            // A still screen sends nothing new: draw the last one again.
            handler?.post { draw(false) }
        }

    private val thread = HandlerThread("kagaz-tv-picture")
    private var handler: Handler? = null
    private var dpy: EGLDisplay = EGL14.EGL_NO_DISPLAY
    private var ctx: EGLContext = EGL14.EGL_NO_CONTEXT
    private var win: EGLSurface = EGL14.EGL_NO_SURFACE
    private var screen: SurfaceTexture? = null
    private var input: Surface? = null
    private var screenTex = 0
    private var plain = 0 // the screen as it is
    private var downScreen = 0 // the screen, averaged down
    private var rounded = 0 // a texture, averaged down, its colours rounded
    private var soft = 0 // a small texture spread out, smoothly
    private var frost = 0 // the spread texture over the whole picture, dimmed
    private val small = IntArray(2) // textures: an eighth, then a sixty-fourth
    private val smallFbo = IntArray(2)
    private val smallSize = arrayOf(intArrayOf(0, 0), intArrayOf(0, 0))
    private val matrix = FloatArray(16)
    private val same = floatArrayOf(1f, 0f, 0f, 0f, 0f, 1f, 0f, 0f, 0f, 0f, 1f, 0f, 0f, 0f, 0f, 1f)
    private var seen = false
    private var live = false
    private lateinit var corners: FloatBuffer

    /** Sets up, and gives the surface the screen is to be mirrored onto.
     *  Throws when the phone's graphics cannot do it. */
    fun start(): Surface {
        thread.start()
        val h = Handler(thread.looper)
        handler = h
        val ready = CountDownLatch(1)
        var failed: Throwable? = null
        h.post {
            try {
                setUp(h)
            } catch (e: Throwable) {
                failed = e
            }
            ready.countDown()
        }
        if (!ready.await(5, TimeUnit.SECONDS)) throw IllegalStateException("the phone's graphics did not answer")
        failed?.let {
            stop()
            throw it
        }
        return input ?: throw IllegalStateException("no surface for the screen")
    }

    fun stop() {
        val h = handler
        handler = null
        if (h != null) {
            val done = CountDownLatch(1)
            h.post {
                tearDown()
                done.countDown()
            }
            try { done.await(1, TimeUnit.SECONDS) } catch (e: Throwable) {}
        }
        thread.quitSafely()
    }

    private fun check(what: String) {
        val e = EGL14.eglGetError()
        if (e != EGL14.EGL_SUCCESS) throw IllegalStateException("$what failed (EGL ${Integer.toHexString(e)})")
    }

    private fun setUp(h: Handler) {
        dpy = EGL14.eglGetDisplay(EGL14.EGL_DEFAULT_DISPLAY)
        val version = IntArray(2)
        if (!EGL14.eglInitialize(dpy, version, 0, version, 1)) throw IllegalStateException("no graphics display")
        val wanted = intArrayOf(
            EGL14.EGL_RED_SIZE, 8, EGL14.EGL_GREEN_SIZE, 8, EGL14.EGL_BLUE_SIZE, 8, EGL14.EGL_ALPHA_SIZE, 8,
            EGL14.EGL_RENDERABLE_TYPE, EGL14.EGL_OPENGL_ES2_BIT,
            0x3142, 1, // EGL_RECORDABLE_ANDROID: fit for an encoder
            EGL14.EGL_NONE
        )
        val configs = arrayOfNulls<EGLConfig>(1)
        val count = IntArray(1)
        if (!EGL14.eglChooseConfig(dpy, wanted, 0, configs, 0, 1, count, 0) || count[0] == 0) {
            throw IllegalStateException("no graphics setup fit for the encoder")
        }
        ctx = EGL14.eglCreateContext(dpy, configs[0], EGL14.EGL_NO_CONTEXT, intArrayOf(EGL14.EGL_CONTEXT_CLIENT_VERSION, 2, EGL14.EGL_NONE), 0)
        check("the graphics context")
        win = EGL14.eglCreateWindowSurface(dpy, configs[0], out, intArrayOf(EGL14.EGL_NONE), 0)
        check("drawing onto the encoder")
        if (!EGL14.eglMakeCurrent(dpy, win, win, ctx)) throw IllegalStateException("the graphics context would not start")

        corners = ByteBuffer.allocateDirect(16 * 4).order(ByteOrder.nativeOrder()).asFloatBuffer()
        // x, y, then where in the picture: the four corners as a strip.
        corners.put(floatArrayOf(-1f, -1f, 0f, 0f, 1f, -1f, 1f, 0f, -1f, 1f, 0f, 1f, 1f, 1f, 1f, 1f)).position(0)

        val vertex = """
            attribute vec2 aAt; attribute vec2 aIn; uniform mat4 uTurn; varying vec2 vIn;
            void main() { gl_Position = vec4(aAt, 0.0, 1.0); vIn = (uTurn * vec4(aIn, 0.0, 1.0)).xy; }
        """
        val ext = "#extension GL_OES_EGL_image_external : require\nprecision mediump float; uniform samplerExternalOES uPic;"
        val two = "precision mediump float; uniform sampler2D uPic;"
        val asIs = " varying vec2 vIn; void main() { gl_FragColor = texture2D(uPic, vIn); }"
        // Sixteen samples, each itself the mean of four neighbours: the
        // mean of an 8 by 8 block.
        val mean = """
            varying vec2 vIn; uniform vec2 uStep;
            void main() {
              vec4 s = vec4(0.0);
              for (int x = 0; x < 4; x++) { for (int y = 0; y < 4; y++) {
                s += texture2D(uPic, vIn + (vec2(float(x), float(y)) * 2.0 - 3.0) * uStep);
              } }
              gl_FragColor = s / 16.0;
            }
        """
        // The same mean, each colour then rounded to 64 steps: the small
        // differences that finer steps would carry are gone.
        val meanRounded = mean.replace("gl_FragColor = s / 16.0;", "gl_FragColor = floor(s / 16.0 * 63.0 + 0.5) / 63.0;")
        // Spreading out: eight samples around each point, the near ones
        // counted twice, so patches run into each other without edges.
        val spreadSoft = """
            varying vec2 vIn; uniform vec2 uStep;
            void main() {
              vec2 h = uStep * 0.5;
              vec4 s = texture2D(uPic, vIn + vec2(-h.x * 2.0, 0.0)) + texture2D(uPic, vIn + vec2(h.x * 2.0, 0.0))
                     + texture2D(uPic, vIn + vec2(0.0, -h.y * 2.0)) + texture2D(uPic, vIn + vec2(0.0, h.y * 2.0))
                     + 2.0 * (texture2D(uPic, vIn + vec2(-h.x, h.y)) + texture2D(uPic, vIn + vec2(h.x, h.y))
                            + texture2D(uPic, vIn + vec2(-h.x, -h.y)) + texture2D(uPic, vIn + vec2(h.x, -h.y)));
              gl_FragColor = s / 12.0;
            }
        """
        // The glass: a little of the colour taken out, and a dim tint
        // over it, as the phone's own panels have.
        val glass = """
            varying vec2 vIn;
            void main() {
              vec3 c = texture2D(uPic, vIn).rgb;
              float grey = dot(c, vec3(0.299, 0.587, 0.114));
              c = mix(c, vec3(grey), 0.25);
              gl_FragColor = vec4(mix(c, vec3(0.13, 0.14, 0.15), 0.38), 1.0);
            }
        """
        plain = program(vertex, ext + asIs)
        downScreen = program(vertex, ext + mean)
        rounded = program(vertex, two + meanRounded)
        soft = program(vertex, two + spreadSoft)
        frost = program(vertex, two + glass)

        val t = IntArray(1)
        GLES20.glGenTextures(1, t, 0)
        screenTex = t[0]
        GLES20.glBindTexture(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, screenTex)
        GLES20.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES20.GL_TEXTURE_MIN_FILTER, GLES20.GL_LINEAR)
        GLES20.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES20.GL_TEXTURE_MAG_FILTER, GLES20.GL_LINEAR)
        GLES20.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES20.GL_TEXTURE_WRAP_S, GLES20.GL_CLAMP_TO_EDGE)
        GLES20.glTexParameteri(GLES11Ext.GL_TEXTURE_EXTERNAL_OES, GLES20.GL_TEXTURE_WRAP_T, GLES20.GL_CLAMP_TO_EDGE)

        smallSize[0] = intArrayOf(maxOf(width / 8, 2), maxOf(height / 8, 2))
        smallSize[1] = intArrayOf(maxOf(width / 64, 2), maxOf(height / 64, 2))
        GLES20.glGenTextures(2, small, 0)
        GLES20.glGenFramebuffers(2, smallFbo, 0)
        for (i in 0..1) {
            GLES20.glBindTexture(GLES20.GL_TEXTURE_2D, small[i])
            GLES20.glTexImage2D(GLES20.GL_TEXTURE_2D, 0, GLES20.GL_RGBA, smallSize[i][0], smallSize[i][1], 0, GLES20.GL_RGBA, GLES20.GL_UNSIGNED_BYTE, null)
            GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_MIN_FILTER, GLES20.GL_LINEAR)
            GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_MAG_FILTER, GLES20.GL_LINEAR)
            GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_WRAP_S, GLES20.GL_CLAMP_TO_EDGE)
            GLES20.glTexParameteri(GLES20.GL_TEXTURE_2D, GLES20.GL_TEXTURE_WRAP_T, GLES20.GL_CLAMP_TO_EDGE)
            GLES20.glBindFramebuffer(GLES20.GL_FRAMEBUFFER, smallFbo[i])
            GLES20.glFramebufferTexture2D(GLES20.GL_FRAMEBUFFER, GLES20.GL_COLOR_ATTACHMENT0, GLES20.GL_TEXTURE_2D, small[i], 0)
            if (GLES20.glCheckFramebufferStatus(GLES20.GL_FRAMEBUFFER) != GLES20.GL_FRAMEBUFFER_COMPLETE) {
                throw IllegalStateException("the phone's graphics cannot draw the hidden picture")
            }
        }
        GLES20.glBindFramebuffer(GLES20.GL_FRAMEBUFFER, 0)

        val st = SurfaceTexture(screenTex)
        st.setDefaultBufferSize(width, height)
        st.setOnFrameAvailableListener({ draw(true) }, h)
        screen = st
        input = Surface(st)
        live = true
    }

    private fun program(vertex: String, fragment: String): Int {
        fun shader(kind: Int, text: String): Int {
            val s = GLES20.glCreateShader(kind)
            GLES20.glShaderSource(s, text)
            GLES20.glCompileShader(s)
            val ok = IntArray(1)
            GLES20.glGetShaderiv(s, GLES20.GL_COMPILE_STATUS, ok, 0)
            if (ok[0] == 0) throw IllegalStateException("a drawing program did not build: ${GLES20.glGetShaderInfoLog(s)}")
            return s
        }
        val p = GLES20.glCreateProgram()
        GLES20.glAttachShader(p, shader(GLES20.GL_VERTEX_SHADER, vertex))
        GLES20.glAttachShader(p, shader(GLES20.GL_FRAGMENT_SHADER, fragment))
        GLES20.glBindAttribLocation(p, 0, "aAt")
        GLES20.glBindAttribLocation(p, 1, "aIn")
        GLES20.glLinkProgram(p)
        val ok = IntArray(1)
        GLES20.glGetProgramiv(p, GLES20.GL_LINK_STATUS, ok, 0)
        if (ok[0] == 0) throw IllegalStateException("a drawing program did not link: ${GLES20.glGetProgramInfoLog(p)}")
        return p
    }

    /** One pass: this texture, through this program, onto this target. */
    private fun pass(program: Int, target: Int, texture: Int, fbo: Int, w: Int, h: Int, turn: FloatArray, stepX: Float, stepY: Float) {
        GLES20.glBindFramebuffer(GLES20.GL_FRAMEBUFFER, fbo)
        GLES20.glViewport(0, 0, w, h)
        GLES20.glUseProgram(program)
        GLES20.glActiveTexture(GLES20.GL_TEXTURE0)
        GLES20.glBindTexture(target, texture)
        GLES20.glUniform1i(GLES20.glGetUniformLocation(program, "uPic"), 0)
        GLES20.glUniformMatrix4fv(GLES20.glGetUniformLocation(program, "uTurn"), 1, false, turn, 0)
        val step = GLES20.glGetUniformLocation(program, "uStep")
        if (step >= 0) GLES20.glUniform2f(step, stepX, stepY)
        corners.position(0)
        GLES20.glVertexAttribPointer(0, 2, GLES20.GL_FLOAT, false, 16, corners)
        GLES20.glEnableVertexAttribArray(0)
        corners.position(2)
        GLES20.glVertexAttribPointer(1, 2, GLES20.GL_FLOAT, false, 16, corners)
        GLES20.glEnableVertexAttribArray(1)
        GLES20.glDrawArrays(GLES20.GL_TRIANGLE_STRIP, 0, 4)
    }

    private fun draw(fresh: Boolean) {
        if (!live) return
        val st = screen ?: return
        try {
            if (fresh) {
                st.updateTexImage()
                st.getTransformMatrix(matrix)
                seen = true
            }
            if (!seen) return
            val ext = GLES11Ext.GL_TEXTURE_EXTERNAL_OES
            if (hidden) {
                // Down to an eighth, down to a sixty-fourth (rounded), back
                // up to an eighth smoothly, then over the whole picture.
                pass(downScreen, ext, screenTex, smallFbo[0], smallSize[0][0], smallSize[0][1], matrix, 1f / width, 1f / height)
                pass(rounded, GLES20.GL_TEXTURE_2D, small[0], smallFbo[1], smallSize[1][0], smallSize[1][1], same, 1f / smallSize[0][0], 1f / smallSize[0][1])
                pass(soft, GLES20.GL_TEXTURE_2D, small[1], smallFbo[0], smallSize[0][0], smallSize[0][1], same, 1f / smallSize[1][0], 1f / smallSize[1][1])
                pass(frost, GLES20.GL_TEXTURE_2D, small[0], 0, width, height, same, 0f, 0f)
            } else {
                pass(plain, ext, screenTex, 0, width, height, matrix, 0f, 0f)
            }
            EGLExt.eglPresentationTimeANDROID(dpy, win, System.nanoTime())
            EGL14.eglSwapBuffers(dpy, win)
        } catch (e: Throwable) {
            Log.w("KagazTv", "a picture was not drawn", e)
        }
    }

    private fun tearDown() {
        live = false
        try { screen?.setOnFrameAvailableListener(null) } catch (e: Throwable) {}
        try { input?.release() } catch (e: Throwable) {}
        try { screen?.release() } catch (e: Throwable) {}
        input = null
        screen = null
        if (dpy != EGL14.EGL_NO_DISPLAY) {
            EGL14.eglMakeCurrent(dpy, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_CONTEXT)
            if (win != EGL14.EGL_NO_SURFACE) EGL14.eglDestroySurface(dpy, win)
            if (ctx != EGL14.EGL_NO_CONTEXT) EGL14.eglDestroyContext(dpy, ctx)
            EGL14.eglReleaseThread()
        }
        win = EGL14.EGL_NO_SURFACE
        ctx = EGL14.EGL_NO_CONTEXT
        dpy = EGL14.EGL_NO_DISPLAY
    }
}
