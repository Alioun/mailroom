import plan from '../../resources/gmail-route-plan.json'

export const gmailRoutePlan = plan
export type RouteLanguage = 'en' | 'de'

export function gmailRouteText(lang: RouteLanguage): string {
  const copy = plan[lang]
  return [
    copy.title,
    copy.boundary,
    copy.private,
    copy.summary,
    ...plan.stages.map((stage, index) => `${index + 1}. ${stage[lang].title}\n${stage[lang].body}`),
    copy.next,
    ...plan.sources.map(source => `${source.label}: ${source.url}`),
    'status=preparation_only provider_changes=0 evidence=unverified',
  ].join('\n\n') + '\n'
}
