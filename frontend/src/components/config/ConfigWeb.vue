<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { cloneDeep } from 'es-toolkit/object'

import GenericModal from '@/components/utils/GenericModal.vue'

import { authFetch } from '@/composables/authFetch'
import { WEB_STEP_ACTIONS, WEB_STEP_FIELDS, newWebPage, newWebStep, webSource } from '@/composables/webPages'
import { useConfig } from '@/stores/config'
import { useIndex } from '@/stores/index'

const { t } = useI18n()
const configStore = useConfig()
const indexStore = useIndex()

const pages = ref<WebPage[]>([])
const page = ref<WebPage>(newWebPage())
const selectedId = ref(0)
const testing = ref(false)
const testResult = ref<WebPageTest>()
const showDeleteModal = ref(false)

const channel = computed(() => configStore.channels[configStore.i]?.id)
const jsonHeaders = { 'Content-Type': 'application/json' }

onMounted(loadPages)
watch(channel, loadPages)

function errorMessage(error: unknown) {
    return error instanceof Error ? error.message : String(error)
}

async function loadPages() {
    if (!channel.value) return

    try {
        pages.value = await authFetch<WebPage[]>(`/api/web/${channel.value}`)
    } catch (error) {
        indexStore.msgAlert('error', errorMessage(error), 4)
        return
    }

    selectPage(pages.value.some((item) => item.id === selectedId.value) ? selectedId.value : 0)
}

function selectPage(id: number) {
    const stored = pages.value.find((item) => item.id === id)

    selectedId.value = stored?.id ?? 0
    page.value = stored ? cloneDeep(stored) : newWebPage()
    testResult.value = undefined
}

function addStep() {
    page.value.steps.push(newWebStep())
}

function moveStep(index: number, offset: number) {
    const target = index + offset
    const steps = page.value.steps
    const step = steps[index]

    if (!step || target < 0 || target >= steps.length) return

    steps.splice(index, 1)
    steps.splice(target, 0, step)
}

function valuePlaceholder(action: WebStepAction) {
    switch (action) {
        case 'press':
            return t('web.valuePress')
        case 'goto':
            return 'https://'
        case 'script':
            return 'JavaScript'
        default:
            return t('web.valueType')
    }
}

async function savePage() {
    if (!channel.value) return

    try {
        if (selectedId.value) {
            await authFetch(`/api/web/${channel.value}/${selectedId.value}`, {
                method: 'PUT',
                headers: jsonHeaders,
                body: JSON.stringify(page.value),
            })
        } else {
            const created = await authFetch<WebPage>(`/api/web/${channel.value}`, {
                method: 'POST',
                headers: jsonHeaders,
                body: JSON.stringify(page.value),
            })

            selectedId.value = created.id
        }

        indexStore.msgAlert('success', t('web.saved'), 2)
        await loadPages()
    } catch (error) {
        indexStore.msgAlert('error', errorMessage(error), 4)
    }
}

async function deletePage(confirmed: boolean) {
    showDeleteModal.value = false

    if (!confirmed || !channel.value || !selectedId.value) return

    try {
        await authFetch(`/api/web/${channel.value}/${selectedId.value}`, { method: 'DELETE' })
        indexStore.msgAlert('success', t('web.deleted'), 2)
        selectedId.value = 0
        await loadPages()
    } catch (error) {
        indexStore.msgAlert('error', errorMessage(error), 4)
    }
}

async function testPage() {
    if (!channel.value || testing.value) return

    testing.value = true
    testResult.value = undefined

    try {
        testResult.value = await authFetch<WebPageTest>(`/api/web/${channel.value}/test`, {
            method: 'POST',
            headers: jsonHeaders,
            body: JSON.stringify(page.value),
        })
    } catch (error) {
        indexStore.msgAlert('error', errorMessage(error), 4)
    } finally {
        testing.value = false
    }
}
</script>

<template>
    <div class="w-full max-w-300 xs:pe-8">
        <h2 class="pt-3 text-3xl">{{ t('web.title') }}</h2>
        <p class="mt-3 max-w-3xl text-sm text-base-content/70">{{ t('web.help') }}</p>

        <div class="mt-6 flex max-w-3xl items-end gap-2">
            <label class="fieldset grow">
                <span class="fieldset-legend">{{ t('web.page') }}</span>
                <select
                    :value="selectedId"
                    class="select select-sm w-full"
                    @change="selectPage(Number(($event.target as HTMLSelectElement).value))"
                >
                    <option :value="0">{{ t('web.newPage') }}</option>
                    <option v-for="item in pages" :key="item.id" :value="item.id">{{ item.name }}</option>
                </select>
            </label>
        </div>

        <form class="mt-2 max-w-5xl" @submit.prevent="savePage">
            <div class="grid max-w-3xl gap-x-4 sm:grid-cols-2">
                <label class="fieldset">
                    <span class="fieldset-legend">{{ t('web.name') }}</span>
                    <input v-model="page.name" type="text" class="input input-sm w-full" maxlength="255" required />
                </label>
                <label class="fieldset">
                    <span class="fieldset-legend">{{ t('web.url') }}</span>
                    <input v-model="page.url" type="url" class="input input-sm w-full" required />
                </label>
                <label class="fieldset">
                    <span class="fieldset-legend">{{ t('web.zoom') }}</span>
                    <input
                        v-model.number="page.zoom"
                        type="number"
                        min="0.25"
                        max="4"
                        step="0.05"
                        class="input input-sm w-full"
                        required
                    />
                    <span class="label text-wrap">{{ t('web.zoomHelp') }}</span>
                </label>
                <label class="fieldset">
                    <span class="fieldset-legend">{{ t('web.preroll') }}</span>
                    <input
                        v-model.number="page.preroll"
                        type="number"
                        min="5"
                        max="300"
                        step="1"
                        class="input input-sm w-full"
                        required
                    />
                    <span class="label text-wrap">{{ t('web.prerollHelp') }}</span>
                </label>
            </div>

            <fieldset class="fieldset mt-2">
                <label class="fieldset-label text-base-content">
                    <input v-model="page.audio" type="checkbox" class="checkbox" />
                    {{ t('web.audio') }}
                </label>
            </fieldset>

            <h3 class="mt-6 text-xl">{{ t('web.steps') }}</h3>
            <p class="mt-1 max-w-3xl text-sm text-base-content/70">{{ t('web.stepsHelp') }}</p>

            <div
                v-for="(step, index) in page.steps"
                :key="index"
                class="mt-3 rounded-box border border-base-content/20 p-3"
                :class="testResult?.failed_step === index + 1 && 'border-error'"
            >
                <div class="grid items-end gap-2 md:grid-cols-[2rem_11rem_1fr_1fr_6rem]">
                    <div class="pb-2 text-center font-bold text-base-content/60">{{ index + 1 }}</div>
                    <label class="fieldset py-0">
                        <span class="fieldset-legend">{{ t('web.action') }}</span>
                        <select v-model="step.action" class="select select-sm w-full">
                            <option v-for="action in WEB_STEP_ACTIONS" :key="action" :value="action">
                                {{ t(`web.actions.${action}`) }}
                            </option>
                        </select>
                    </label>
                    <label class="fieldset py-0" :class="!WEB_STEP_FIELDS[step.action].selector && 'invisible'">
                        <span class="fieldset-legend">{{ t('web.selector') }}</span>
                        <input
                            v-model="step.selector"
                            type="text"
                            class="input input-sm w-full font-mono"
                            :placeholder="t('web.selectorPlaceholder')"
                            :disabled="!WEB_STEP_FIELDS[step.action].selector"
                        />
                    </label>
                    <label class="fieldset py-0" :class="!WEB_STEP_FIELDS[step.action].value && 'invisible'">
                        <span class="fieldset-legend">{{ t('web.value') }}</span>
                        <input
                            v-model="step.value"
                            :type="step.secret ? 'password' : 'text'"
                            autocomplete="off"
                            class="input input-sm w-full"
                            :placeholder="valuePlaceholder(step.action)"
                            :disabled="!WEB_STEP_FIELDS[step.action].value"
                        />
                    </label>
                    <label class="fieldset py-0" :class="!WEB_STEP_FIELDS[step.action].seconds && 'invisible'">
                        <span class="fieldset-legend">{{ t('web.seconds') }}</span>
                        <input
                            v-model.number="step.seconds"
                            type="number"
                            min="0"
                            max="300"
                            step="0.5"
                            class="input input-sm w-full"
                            :disabled="!WEB_STEP_FIELDS[step.action].seconds"
                        />
                    </label>
                </div>
                <div class="mt-2 flex flex-wrap items-center gap-4 ps-10">
                    <label class="fieldset-label text-base-content">
                        <input v-model="step.optional" type="checkbox" class="checkbox checkbox-sm" />
                        {{ t('web.optional') }}
                    </label>
                    <label v-if="WEB_STEP_FIELDS[step.action].value" class="fieldset-label text-base-content">
                        <input v-model="step.secret" type="checkbox" class="checkbox checkbox-sm" />
                        {{ t('web.secret') }}
                    </label>
                    <div class="join ms-auto">
                        <button
                            type="button"
                            class="btn btn-sm join-item"
                            :title="t('web.moveUp')"
                            :disabled="index === 0"
                            @click="moveStep(index, -1)"
                        >
                            <i class="bi-arrow-up" />
                        </button>
                        <button
                            type="button"
                            class="btn btn-sm join-item"
                            :title="t('web.moveDown')"
                            :disabled="index === page.steps.length - 1"
                            @click="moveStep(index, 1)"
                        >
                            <i class="bi-arrow-down" />
                        </button>
                        <button
                            type="button"
                            class="btn btn-sm join-item"
                            :title="t('web.removeStep')"
                            @click="page.steps.splice(index, 1)"
                        >
                            <i class="bi-trash" />
                        </button>
                    </div>
                </div>
            </div>

            <button type="button" class="btn btn-sm mt-3" @click="addStep">
                <i class="bi-plus-lg" />
                {{ t('web.addStep') }}
            </button>

            <div class="mt-6 flex flex-wrap items-center gap-2">
                <button class="btn btn-primary" type="submit">{{ t('config.save') }}</button>
                <button class="btn" type="button" :disabled="testing" @click="testPage">
                    <span v-if="testing" class="loading loading-spinner loading-sm" />
                    {{ testing ? t('web.testing') : t('web.test') }}
                </button>
                <button v-if="selectedId" class="btn btn-outline" type="button" @click="showDeleteModal = true">
                    {{ t('web.delete') }}
                </button>
                <span v-if="selectedId" class="ms-2 text-sm text-base-content/70">
                    {{ t('web.playlistSource') }}: <code>{{ webSource(selectedId) }}</code>
                </span>
            </div>
        </form>

        <div v-if="testResult" class="mt-6 max-w-3xl pb-8">
            <div role="alert" class="alert" :class="testResult.success ? 'alert-success' : 'alert-error'">
                <span v-if="testResult.success">{{ t('web.testSuccess') }}</span>
                <span v-else-if="testResult.failed_step">
                    {{ t('web.testFailedStep', { step: testResult.failed_step, error: testResult.error ?? '' }) }}
                </span>
                <span v-else>{{ t('web.testFailed', { error: testResult.error ?? '' }) }}</span>
            </div>
            <img
                v-if="testResult.screenshot"
                :src="`data:image/jpeg;base64,${testResult.screenshot}`"
                :alt="page.name"
                class="mt-3 w-full rounded border border-base-content/30"
            />
        </div>

        <GenericModal :show="showDeleteModal" :title="t('web.delete')" :modal-action="deletePage">
            <span>{{ t('web.deleteConfirm', { name: page.name }) }}</span>
        </GenericModal>
    </div>
</template>
