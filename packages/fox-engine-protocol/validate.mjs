// Small schema interpreter for the generated wire contract. Generation fails
// if Rust introduces an assertion keyword that this interpreter cannot honor.
export function validateSchema(root, value) {
  function visit(schema, item, path) {
    if (schema === true) return []
    if (schema === false) return [path + ': forbidden value']
    if (schema.$ref) {
      if (!schema.$ref.startsWith('#/')) return [path + ': external schema reference is forbidden']
      const target = schema.$ref.slice(2).split('/').reduce((current, part) => current?.[part.replaceAll('~1', '/').replaceAll('~0', '~')], root)
      return target ? visit(target, item, path) : [path + ': unresolved schema reference']
    }
    const errors = []
    if ('const' in schema && JSON.stringify(item) !== JSON.stringify(schema.const)) errors.push(path + ': invalid constant')
    if (schema.enum && !schema.enum.some(option => JSON.stringify(option) === JSON.stringify(item))) errors.push(path + ': invalid enum value')
    for (const field of ['anyOf', 'oneOf']) if (schema[field]) {
      const matches = schema[field].filter(option => visit(option, item, path).length === 0).length
      if (field === 'oneOf' ? matches !== 1 : matches === 0) errors.push(path + ': invalid union value')
    }
    for (const child of schema.allOf ?? []) errors.push(...visit(child, item, path))
    const types = schema.type === undefined ? [] : [].concat(schema.type)
    const matchesType = type => type === 'null' ? item === null
      : type === 'array' ? Array.isArray(item)
      : type === 'object' ? item !== null && typeof item === 'object' && !Array.isArray(item)
      : type === 'integer' ? Number.isSafeInteger(item)
      : type === 'number' ? typeof item === 'number' && Number.isFinite(item)
      : typeof item === type
    if (types.length && !types.some(matchesType)) return [...errors, path + ': invalid type']
    if (typeof item === 'number') {
      if (schema.minimum !== undefined && item < schema.minimum) errors.push(path + ': below minimum')
      if (schema.maximum !== undefined && item > schema.maximum) errors.push(path + ': above maximum')
    }
    if (typeof item === 'string') {
      const length = [...item].length
      if (schema.minLength !== undefined && length < schema.minLength) errors.push(path + ': too short')
      if (schema.maxLength !== undefined && length > schema.maxLength) errors.push(path + ': too long')
      if (schema.pattern && !new RegExp(schema.pattern).test(item)) errors.push(path + ': invalid pattern')
    }
    if (Array.isArray(item)) {
      if (schema.minItems !== undefined && item.length < schema.minItems) errors.push(path + ': too few items')
      if (schema.maxItems !== undefined && item.length > schema.maxItems) errors.push(path + ': too many items')
      if (schema.items !== undefined) item.forEach((entry, index) => errors.push(...visit(schema.items, entry, path + '[' + index + ']')))
    } else if (item !== null && typeof item === 'object') {
      for (const required of schema.required ?? []) if (!Object.hasOwn(item, required)) errors.push(path + '.' + required + ': required')
      for (const [name, entry] of Object.entries(item)) {
        // JSON.stringify omits undefined optional properties on the wire.
        if (entry === undefined && !schema.required?.includes(name)) continue
        if (Object.hasOwn(schema.properties ?? {}, name)) errors.push(...visit(schema.properties[name], entry, path + '.' + name))
        else if (schema.additionalProperties === false) errors.push(path + '.' + name + ': unknown field')
        else if (typeof schema.additionalProperties === 'object') errors.push(...visit(schema.additionalProperties, entry, path + '.' + name))
      }
    }
    return errors
  }
  return visit(root, value, '$')
}
